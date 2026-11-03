# `quadraui-binding-core` publish policy

**Status:** Decided. 2026-11-03.

**Scope:** Whether/when `quadraui-binding-core` needs `publish = true` and a
crates.io release, given it is currently `publish = false` and consumed only
by in-workspace path dependencies.

---

## Question

`quadraui-binding-core/Cargo.toml` carries `publish = false`. That is correct
today: every binding leaf that depends on it — `quadraui-py`, `quadraui-ffi`,
`quadraui-node` — lives in this workspace and reaches it via a path
dependency (`quadraui-binding-core = { path = "../quadraui-binding-core" }`),
so no registry publish is needed for any of them to build or release.

The open question, surfaced by the `quadraui-node` (#1126) JS-readiness
audit: does that change if a binding leaf is ever extracted into its own
repo — the way `coord-tui` was split out of `code-coordinator` — and if so,
when should the work happen?

## Decision

**Status quo. No action now.** Keep every binding leaf in this workspace,
keep `quadraui-binding-core` as `publish = false`, and defer the publish
question until a leaf extraction is actually proposed.

**If/when a leaf extraction is proposed**, the extracted repo has exactly
two options for reaching `quadraui-binding-core`, and the extraction PR must
pick one explicitly:

1. **Publish `quadraui-binding-core` to crates.io** alongside `quadraui`
   itself, and have the extracted repo depend on the published version
   (same shape as `vimcode`'s dependency on `quadraui` — see the Downstream
   consumers table in the repo-root `CLAUDE.md`). This is the right choice
   if the extracted repo wants the same 0.1.x-bump release cadence `quadraui`
   already has.
2. **Pin a git dependency** (`quadraui-binding-core = { git = "...", rev =
   "<sha>" }`), the same shape `coord-tui` uses for `quadraui` itself. This
   avoids a registry publish entirely and is the right choice if the
   extracted repo wants to track `develop` at an explicit, manually-bumped
   revision rather than a release.

Either way, the extraction PR — not this issue — is what flips
`publish = false` (option 1) or adds the pin (option 2), because only that
PR knows which leaf is extracting and which option its maintainers want.

## Why status quo now

- **No leaf extraction is currently planned.** `quadraui-py`, `quadraui-ffi`
  and `quadraui-node` all live in-workspace today; path dependencies are
  simpler to develop against (no publish-then-bump round trip for every
  `quadraui-binding-core` change) and nothing requires breaking that yet.
- **Publishing early has a real cost with no current payoff.**
  `quadraui-binding-core` is pre-1.0 and under active development alongside
  every binding leaf (see recent `feat(quadraui-node)` / `feat(quadraui-py)`
  commits). Publishing it now would mean versioning and changelog discipline
  on a crate with no external consumer, duplicating the churn `quadraui`
  itself already carries post-1.0-promotion.
- **The decision genuinely is deferrable without risk.** Both of the options
  above (crates.io publish, or a pinned git dependency) are available
  whenever a leaf extraction actually happens; neither requires groundwork
  done today. This note exists so that choice is made deliberately in the
  extraction PR, not improvised under schedule pressure.

## Revisit when

- Any PR proposes extracting `quadraui-py`, `quadraui-ffi`, `quadraui-node`,
  or a future binding leaf into its own repo. That PR picks option 1 or 2
  above and updates this file's **Status** line to reflect what was chosen
  (and why), the same way `BINDING_STRATEGY.md` would be revisited with a
  real consumer in hand.
