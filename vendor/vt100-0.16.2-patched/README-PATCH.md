# Patch notes: vt100 0.16.2 → unicode-width bound loosened

Vendored verbatim from crates.io `vt100 0.16.2` (upstream:
https://github.com/doy/vt100-rust). The **only** change from upstream is in
`Cargo.toml`:

```diff
-unicode-width = "0.2.1"
+unicode-width = "0.2"
```

No source files (`src/**`) were touched.

## Why this exists (quadraui#452)

quadraui bumped vt100 0.15 → 0.16 in commit `96bfafe` (#397) to fix a
`Grid::set_size` cursor-clamp panic. That bump pulled in vt100 0.16's own
`unicode-width` requirement of `^0.2.1` — up from `0.1.10` in vt100 0.15,
which was a different major line and never unified with anything else in
the graph.

`^0.2.1` is mathematically incompatible with **any** consumer that
exactly-pins `unicode-width = "=0.2.0"` — which is exactly what `ratatui
0.29` does upstream, and what vimcode's `Cargo.toml` pins directly (a
separate, deliberate pin — see vimcode's own history for why). This has
nothing to do with what version quadraui itself declares for
`unicode-width` in `quadraui/Cargo.toml`; that declaration is a red
herring. Proof: a minimal crate depending on nothing but
`vt100 = "0.16"` + `ratatui = "=0.29"` fails to resolve with the identical
`unicode-width` error, with quadraui out of the picture entirely.

vt100's actual code only uses `unicode_width::UnicodeWidthChar` (see
`src/cell.rs`, `src/screen.rs`) — stable API present since unicode-width
0.2.0. The `^0.2.1` floor in vt100's manifest appears to be an incidental
side effect of a routine `cargo update` at release time, not a real
functional dependency on anything 0.2.1 added. Loosening the manifest
bound back to `"0.2"` (allowing 0.2.0) carries no known behavioral risk.

## ⚠️ Do not remove this patch without checking first

quadraui already tried "just use crates.io directly" once before for vt100
(commit `96bfafe` removed the #377 vendor shim, believing the "real" 0.16
bump was cleaner) — and that's exactly what reintroduced this conflict.
If you're tempted to drop this `[patch.crates-io]` entry and go back to
crates.io `vt100`, first confirm **one** of:

1. Upstream vt100 has relaxed its own `unicode-width` bound below `0.2.1`
   in a newer release, or
2. vimcode (or whichever consumer) no longer exactly-pins
   `unicode-width` via an exact-pinned `ratatui` (or other) dependency.

Otherwise this patch is load-bearing — removing it silently breaks every
downstream build that combines quadraui's `tui`/`terminal`/`gtk` features
with an exact `unicode-width = "=0.2.0"` pin anywhere in its own
dependency graph.
