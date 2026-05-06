# Primitive Authoring Rules

Read this when adding or changing a primitive.

1. **Declarative description first.** Add the struct + `Layout` +
   `hit_test` to `quadraui/src/primitives/<name>.rs`. Tests for layout
   correctness go at the bottom of that file as inline tests; they must
   not pull in any backend feature.
2. **One source of truth for layout.** Both rasterisers AND tests must
   call the primitive's `layout(...)` — never re-derive bounds inline.
   Where a backend needs additional metrics (like TUI's `cell_quantum`),
   they go on `LayoutMetrics` so the primitive can apply them.
3. **Both backends, same shape.** Add the rasteriser to
   `quadraui/src/tui/<name>.rs` AND `quadraui/src/gtk/<name>.rs`. Even
   if the GTK consumer hasn't been written yet, add a stub rasteriser
   so the primitive's contract is honoured on every backend it claims
   to support.
4. **Paint/click round-trip harness.** Every primitive that has clicks
   must have a paint/click round-trip test in its TUI rasteriser
   (`quadraui/src/tui/<name>.rs::tests`). The pattern: paint into a
   `ratatui::Buffer`, find painted glyphs, hit_test those exact
   coordinates, assert paint and click identify the same region.
   Examples to copy: `tui::multi_section_view::tests`,
   `tui::tree::tests`. The harness must catch the bug class it's
   designed for — verify by temporarily mutating the rasteriser to
   break the contract; the harness should fail. Restore + commit only
   when both sides round-trip cleanly.
5. **Public layout helper.** Each backend exposes a public layout
   helper (e.g. `tui_msv_layout`, `tui_tree_layout`) so consumers can
   drive hit-testing without re-deriving metrics. The rasteriser uses
   this same helper internally — paint and consumer-driven hit_test
   consume one source of truth.
6. **Test state-derived paint geometry.** Whenever a painter computes
   a position from primitive state — scrollbar thumb position from
   `(scroll_offset, content_rows, viewport_rows)`, focus ring from
   `has_focus`, badge offset from text length, animation phase from
   a tick counter — write a test that paints at a known state and
   asserts the painted indicator lands where the formula predicts.
   The paint/click harness covers coordinate-system drift; this rule
   covers paint-formula bugs (e.g. "thumb hardcoded at top of gutter
   instead of computed from scroll_offset"). Verify empirically by
   mutating the formula and observing the test fail. See *Coverage
   taxonomy* under *Testing* for the full bug-class breakdown.
7. **Add the primitive to the `Backend` trait.** Every primitive MUST
   have a `Backend::draw_<name>` (and where applicable, `Backend::<name>_layout`)
   method. Per `BACKEND_TRAIT_PROPOSAL.md` section 4, adding a primitive is
   an intentional breaking change to the trait — every backend
   implementer sees the new method as a compile error and fills in
   their rasteriser. **No primitive ships with TUI/GTK free-function
   rasterisers but no trait coverage.** That's how the
   *Cross-backend portability commitment* stays load-bearing —
   if a primitive isn't on the trait, downstream consumer code has to
   pick a backend explicitly.

## Primitive maturity levels

A primitive file in `primitives/` is not "shipped" until it has all
three legs: (1) descriptor + layout + hit_test + layout tests, (2) TUI
and GTK rasterisers with paint/click harnesses, (3) `Backend` trait
methods. Primitives with only leg (1) are **descriptors** — their shape
is real and tested, but consumers cannot adopt them yet because there's
nothing to paint. As of 2026-05-03, all previously descriptor-only
primitives have shipped with full rasteriser + Backend trait coverage.
Don't delete descriptors — the layout + hit_test work is real and
reusable. Do prioritise adding rasterisers for any descriptor that
blocks a consumer's bespoke-paint elimination.
