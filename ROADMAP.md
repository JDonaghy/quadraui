# quadraui — Roadmap

> **The source of truth for *order*.** [`GOAL.md`](GOAL.md) says what quadraui
> is trying to be; this file says in what sequence the remaining work lands and
> what each release unlocks. GitHub is the source of truth for issue *status* —
> this file links epics and names gates, and deliberately does not track
> individual issues, which would go stale here.
>
> Derived from the 2026-09-26 framework audit
> ([`quadraui/docs/audits/FRAMEWORK_AUDIT_2026-09-26.md`](quadraui/docs/audits/FRAMEWORK_AUDIT_2026-09-26.md)
> §7.2–7.3), which is a dated snapshot; this file is the living version.
>
> _Last updated: 2026-10-05._

## Why this file exists

Epic checklists can only order work *inside* one epic (`{after: #N}` must name
a sibling). The gates that matter most here cross epics — accessibility needs
the owned `Frame` from the widget-model epic; bindings need a stable release —
and nothing else records them. A milestone whose children show as "ready" in
the planner may in fact be gated on another epic. **Check the gate table below
before queuing anything from milestones #19 or #20.**

## Release train

| Release | Contents | Breaking? | Status |
|---|---|---|---|
| **v0.1.0** | First crates.io publish (#783). Release prep, `#[non_exhaustive]` descriptors, deprecation removal, docs truth pass. | — | Prepared on `develop`; tag + `cargo publish` pending (operator step, #1111). |
| **v0.2** | The widget-model breaking batch (#1095): the owned `Frame` tree (the remaining half of #1099), interned `WidgetId` (#1105), retained descriptor store (#1104). | **Yes** | Not started; waits for v0.1.0 so consumers can pin `release/0.1.x`. The owned-`Frame` follow-up issue is not yet filed — #1099 stays open until it is. |
| **v0.3** | Accessibility (#1309) and the remaining app capabilities (#788). Descriptor changes are additive only. | No (target) | Filed; gated on v0.2. |
| **v0.4+** | Language bindings (#1096). | No | Gated on the stability rule below. |
| **1.0** | The 1.0 bar is met (below), then a stability pledge. | — | — |

Already landed on `develop` ahead of the train: `step()`/`pump()` non-blocking
runner (#1100), one unit contract for `Rect` (#1098), split `Backend` with a
public paint seam (#1101), optional flex/grid layout (#1103), proportional-font
metrics (#1132), styling beyond colours (#1133), `Canvas` (#1102), and the
first half of #1099 — every primitive now has a `Surface`/`FrameZone`
variant and `FrameHitMap` is serializable (#1298, additive).

## Cross-epic gates

| Work | Epic / milestone | May start when |
|---|---|---|
| Widget-model breaking batch (#1099, #1104, #1105) | #1095 / #18 | v0.1.0 is published. |
| Accessibility | #1309 / #20 | v0.1.0 is published **and** #1099 + #1105 have landed. |
| Language bindings (C ABI, Python, C#, TypeScript) | #1096 / #19 | Primitive descriptors have survived **one minor release unchanged** — v0.3 at the earliest. Every binding multiplies the cost of every later breaking change. |
| Lua bridge (#115) | #1096 / #19 | #1104 has landed. Exception to the stability rule: it lives in this workspace, so primitive changes and bridge updates ship in the same PR. |
| JSON-over-stdio IPC (#118) | #1096 / #19 | Decide first whether the C ABI (#1123) supersedes it; it predates the audit. |
| IME (#900) | #788 / #17 | Independent of the widget model; needs its four backend integrations filed from `quadraui/docs/IME_INPUT_PROPOSAL.md`. |
| Rows provider + damage rects (#1121) | #788 / #17 | Design together with the descriptor store (#1104) — they solve the same per-frame rebuild cost. |

## The 1.0 bar

What must be true before recommending quadraui to an outside developer
(audit §7.3).

| # | Criterion | Where it stands |
|---|---|---|
| 1 | Published on crates.io with hosted docs; one minor release with no breaking change to primitive descriptors | v0.1.0 prepared, unpublished; stability release is v0.3 at the earliest |
| 2 | AccessKit on GTK, macOS and Windows with a screen-reader smoke test in CI | Epic #1309 |
| 3 | IME composition on all three GUI backends | #900 — design written, backends unbuilt |
| 4 | A public drawing surface and a layout module | ✅ `Canvas` (#1102), `layout` feature (#1103) |
| 5 | `Rect` in one documented unit on every backend | ✅ #1098 (D-016) |
| 6 | Windows gating in conformance, and one shipped app on it | Gating ✅ (#784); no shipped Windows app yet |
| 7 | Two production consumers not owned by the maintainer, or one with real end users | Not yet — vimcode and coord-tui are both maintainer-owned |
| 8 | Benchmarks in CI for 100k-row and 4k-line cases | ✅ #1118 |
| 9 | Docs verified against code in the same PR as changes | ✅ `quadraui/tests/readme_truth.rs` (#798, #1107) |
| 10 | A packaging guide per platform | #1122 |

RTL/bidi may stay a documented non-goal past 1.0; accessibility and IME may
not.

## Not on the train

Filed but deliberately unscheduled: multi-window (#1120), dock badge/progress
(#1241), print/PDF (#1242). They slot in whenever a consumer needs them; none
blocks 1.0.
