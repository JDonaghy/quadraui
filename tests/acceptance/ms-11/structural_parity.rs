// Sealed acceptance slice — ms-11 / issue #542 ("cross_backend_parity
// asserts logical state only — a backend that silently drops chrome
// (borders, titles, scrollbars) passes; needs a structural tier").
//
// Authored independently from the implementation, against
// `tests/acceptance/ms-11/contract.md` §4. `include!`d from
// `quadraui/tests/acceptance.rs` below its SEALED marker.
//
// ── The two tiers (contract §4b, #542 acceptance bullet 3) ────────────────
//
// #542's third acceptance bullet requires that "behavioural and structural
// failures are distinguishable from the test output alone". This slice
// therefore keeps them in *separate test functions*, named and messaged so
// the tier is legible from a bare libtest line:
//
//   behavioural_parity_*  — tier A. Logical state: does the text reach the
//                           screen, does the app agree it is showing.
//                           This is what `tests/cross_backend_parity.rs`
//                           already asserts, and every assertion here is
//                           GREEN on today's tree *by design* — it is the
//                           control half. A backend that drops chrome
//                           passes tier A; that is precisely the gap.
//
//   structural_parity_*   — tier B. The emitted surface set and its
//                           geometry, read from
//                           `ConformanceDriver::inventory()`
//                           (`FrameInventory { text_runs, zones }`, #490).
//                           These are RED today: `draw_tooltip` registers
//                           no zone on either backend, so the frame reports
//                           no tooltip surface at all.
//
// Every failure message below is prefixed `BEHAVIOURAL PARITY (tier A)` or
// `STRUCTURAL PARITY (tier B)` so the two never have to be told apart by
// reading the assertion.
//
// ── Why the observable is `inventory()`, not a screen diff ────────────────
//
// Contract §4b pins the structural observable to `FrameInventory`: "Both
// drivers already implement `ConformanceDriver::inventory() -> FrameInventory`
// (`text_runs`, `zones`). The structural tier asserts on the **inventory**,
// not on text presence." A grid diff cannot serve here — GTK is a pixel
// backend with no cell grid, and the whole point of the tier is to compare
// the two backends against *each other*, in units neither one shares.
//
// Consequently every cross-backend number below is either a *set* of
// surface ids (unit-free) or a *ratio* between two rects the same backend
// reported in the same frame (unit-free by construction) — never a raw
// cell or pixel count. That is #542's ask 4, "where the two backends
// should legitimately differ (pixel vs cell metrics), make that explicit
// in the assertion rather than absent from it": the metric difference is
// divided out where it is legitimate, and asserted on where it is not.
//
// ── The #541 case this tier exists to catch (contract §4c note 4) ─────────
//
// GTK's `draw_tooltip` strokes a full 4-sided box; TUI's paints `│` on the
// first and last column only. Same primitive, same call, materially
// different chrome — and `screen_has("Keybindings")` is `true` on both, so
// tier A is green on both. `tooltip_surface_encloses_its_text_on_every_side`
// below is the clause that fails when a backend reports a tooltip surface
// with no top/bottom chrome around its text.

#[cfg(all(feature = "tui", feature = "gtk"))]
mod ms11_542_structural_parity {
    use std::collections::BTreeSet;

    use quadraui::gtk::testing::GtkDriver;
    use quadraui::testing::{ConformanceDriver, FrameInventory, LogicalViewport};
    use quadraui::tui::testing::TuiDriver;
    use quadraui::{
        AppLogic, AppShell, Backend, Color, PanelDefinition, Reaction, Rect, StatusBar,
        StatusBarSegment, Tooltip, TooltipMeasure, TooltipPlacement, UiEvent, WidgetId,
    };

    /// #541's live case, kept verbatim so the clause names the bug it came
    /// from: `screen_has("Keybindings")` was `true` on both backends while
    /// the border divergence shipped.
    const TOOLTIP_TEXT: &str = "Keybindings";
    /// The tooltip's own `WidgetId`. The fixture chooses it, so a slice
    /// asserting on it is naming its own input, not restating an
    /// implementation constant.
    const TOOLTIP_ID: &str = "ms11:542:tooltip";
    /// Painted into a status bar in the same frame — a second primitive
    /// from #542's "cover the primitives that have consumers today" list,
    /// and the element the tooltip is anchored to.
    const ANCHOR_TEXT: &str = "STATUS";

    /// Backend-neutral viewport (`LogicalViewport` is the unit-free
    /// constructor `ConformanceDriver` exposes for exactly this reason:
    /// TUI reads it as cells, GTK scales it to pixels).
    const VIEWPORT: LogicalViewport = LogicalViewport::new(60, 12);

    // ── Fixtures ─────────────────────────────────────────────────────────
    //
    // Defined inline, public quadraui API only, per contract §6: a fixture
    // that needed a new constructor on an `examples/common` type could not
    // compile until that constructor landed, and a slice that fails to
    // compile takes every other milestone's slice down with it instead of
    // failing red on its own clause.

    /// Draws a status bar and a `Tooltip` anchored beneath it — the
    /// minimal frame contract §4b calls for ("a fixture drawing a
    /// `Tooltip`").
    ///
    /// Every rect is expressed in the backend's *own* metrics
    /// (`char_width` / `line_height` / `viewport`), never in literal cells
    /// or pixels, so the identical fixture produces the logically identical
    /// frame on a cell backend and a pixel backend. Writing `Rect::new(4.0,
    /// 4.0, …)` here would silently mean "4 cells" on TUI and "4 pixels"
    /// — a quarter of one character — on GTK.
    struct TooltipFixture;

    impl AppLogic for TooltipFixture {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let vp = backend.viewport();
            let cw = backend.char_width();
            let lh = backend.line_height();
            let viewport = Rect::new(0.0, 0.0, vp.width, vp.height);

            // Status bar across the top row — the tooltip's anchor.
            let anchor = Rect::new(0.0, 0.0, vp.width, lh);
            let bar = StatusBar {
                id: WidgetId::new("ms11:542:status-bar"),
                left_segments: vec![StatusBarSegment {
                    text: format!(" {ANCHOR_TEXT} "),
                    fg: Color::rgb(220, 220, 220),
                    bg: Color::rgb(37, 37, 38),
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            };
            let _ = backend.draw_status_bar(anchor, &bar, None, None);

            let tooltip = Tooltip {
                id: WidgetId::new(TOOLTIP_ID),
                text: TOOLTIP_TEXT.to_string(),
                styled_lines: None,
                placement: TooltipPlacement::Bottom,
                bg: None,
                fg: None,
            };
            // One text line plus a row of chrome above and below it, and
            // room for chrome either side — i.e. a box big enough for a
            // border on all four sides *and* the whole label. Expressed in
            // backend metrics, so it is 3 rows on TUI and 3 line-heights
            // on GTK.
            //
            // The horizontal slack (+4 rather than +2 columns) is
            // deliberate: a box measured to exactly `border + text +
            // border` leaves a backend no room for padding and clips the
            // last glyph, which would make the tier-A control below fail
            // for a reason that has nothing to do with #542. The clauses
            // are relational, so slack costs them nothing.
            let measure = TooltipMeasure::new(
                cw * (TOOLTIP_TEXT.chars().count() as f32 + 4.0),
                lh * 3.0,
            );
            let layout = tooltip.layout(anchor, viewport, measure, lh);
            backend.draw_tooltip(&tooltip, &layout);
        }

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    /// Draws the shell chrome `AppShell` composes — activity bar, sidebar
    /// header/content, resize divider, status bar. #542 ask 3 names the
    /// status bar, sidebar chrome and the terminal divider as primitives
    /// "that have consumers today"; this is the fixture that carries them.
    struct ShellChromeFixture {
        shell: AppShell,
    }

    impl ShellChromeFixture {
        fn new() -> Self {
            Self {
                shell: AppShell::new(
                    vec![
                        PanelDefinition {
                            id: WidgetId::new("panel:explorer"),
                            icon: "E".to_string().into(),
                            tooltip: "Explorer".to_string(),
                            title: "EXPLORER".to_string(),
                        },
                        PanelDefinition {
                            id: WidgetId::new("panel:search"),
                            icon: "S".to_string().into(),
                            tooltip: "Search".to_string(),
                            title: "SEARCH".to_string(),
                        },
                    ],
                    // Sidebar width is stored in `line_height` multiples,
                    // so this is portable across cell and pixel backends.
                    18.0,
                )
                .with_status_bar(),
            }
        }
    }

    impl AppLogic for ShellChromeFixture {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let vp = backend.viewport();
            let _ = self
                .shell
                .render(backend, Rect::new(0.0, 0.0, vp.width, vp.height));
        }

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    // ── Observation helpers ──────────────────────────────────────────────

    /// Paint one frame of `app` on backend `D` and return its inventory.
    ///
    /// Generic over `ConformanceDriver` rather than duplicated per
    /// backend: a shared body cannot accidentally assert something on one
    /// backend it forgot to assert on the other, which is the failure mode
    /// this whole issue is about.
    fn frame<D, A>(app: A) -> FrameInventory
    where
        A: AppLogic,
        D: ConformanceDriver<App = A>,
    {
        D::new_fixture(app, VIEWPORT).inventory()
    }

    /// The frame's **emitted surface set**: every `WidgetId` the backend
    /// registered this frame, as a sorted set of strings. Unit-free, so it
    /// compares directly across a cell backend and a pixel backend — this
    /// is #542 ask 1 ("assert the emitted surface set, not just text").
    fn surfaces(inv: &FrameInventory) -> BTreeSet<String> {
        inv.zones()
            .iter()
            .map(|z| z.id.as_str().to_string())
            .collect()
    }

    /// Bounds of the surface registered under `id`, if the frame emitted
    /// one at all.
    fn surface_bounds(inv: &FrameInventory, id: &str) -> Option<Rect> {
        inv.zones()
            .iter()
            .find(|z| z.id.as_str() == id)
            .map(|z| z.bounds)
    }

    /// Bounds of the first painted text run containing `needle`.
    fn text_bounds(inv: &FrameInventory, needle: &str) -> Option<Rect> {
        inv.text_runs()
            .iter()
            .find(|r| r.text.contains(needle))
            .map(|r| r.bounds)
    }

    /// Both backends' inventories for the tooltip fixture, in a fixed
    /// order so every message below can name them the same way.
    fn tooltip_frames() -> [(&'static str, FrameInventory); 2] {
        [
            ("TuiDriver", frame::<TuiDriver<_>, _>(TooltipFixture)),
            ("GtkDriver", frame::<GtkDriver<_>, _>(TooltipFixture)),
        ]
    }

    fn shell_frames() -> [(&'static str, FrameInventory); 2] {
        [
            (
                "TuiDriver",
                frame::<TuiDriver<_>, _>(ShellChromeFixture::new()),
            ),
            (
                "GtkDriver",
                frame::<GtkDriver<_>, _>(ShellChromeFixture::new()),
            ),
        ]
    }

    // ═══ Tier A — behavioural parity ═════════════════════════════════════
    //
    // GREEN TODAY, DELIBERATELY. This is the control half of contract §4b:
    // "a backend that omits the border fails a structural assertion **while
    // still passing the behavioural one**". If a change ever makes this
    // test red at the same time as the tier-B tests below, the failure is
    // not #542's — something stopped painting the tooltip at all.

    #[test]
    fn behavioural_parity_tooltip_text_reaches_every_backend() {
        for (name, inv) in tooltip_frames() {
            assert!(
                inv.screen_has(TOOLTIP_TEXT),
                "BEHAVIOURAL PARITY (tier A) — contract §4b: {name} did not paint \
                 {TOOLTIP_TEXT:?} at all. This tier is the control: it is expected to be \
                 GREEN even on a backend that drops the tooltip's border, so a failure here \
                 means the tooltip is not being drawn, not that its chrome diverged.\n\
                 painted runs: {:?}",
                inv.text_runs()
                    .iter()
                    .map(|r| r.text.as_str())
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn behavioural_parity_status_bar_text_reaches_every_backend() {
        for (name, inv) in tooltip_frames() {
            assert!(
                inv.screen_has(ANCHOR_TEXT),
                "BEHAVIOURAL PARITY (tier A): {name} did not paint the status-bar text \
                 {ANCHOR_TEXT:?}. Control half — see the tier-A note at the top of this \
                 slice.\npainted runs: {:?}",
                inv.text_runs()
                    .iter()
                    .map(|r| r.text.as_str())
                    .collect::<Vec<_>>()
            );
        }
    }

    // ═══ Tier B — structural parity ══════════════════════════════════════
    //
    // RED TODAY. `draw_tooltip` calls `register_zone` on neither backend,
    // so the tooltip contributes no surface to `inventory().zones()` and
    // the frame is structurally indistinguishable from one where the
    // tooltip was never drawn — which is exactly the blindness #542
    // reports.

    /// Contract §4b, first Required: "for a fixture drawing a `Tooltip`,
    /// both backends report a tooltip zone in `inventory().zones()`".
    #[test]
    fn structural_parity_every_backend_reports_the_tooltip_surface() {
        for (name, inv) in tooltip_frames() {
            let emitted = surfaces(&inv);
            assert!(
                emitted.contains(TOOLTIP_ID),
                "STRUCTURAL PARITY (tier B) — contract §4b: {name} painted a Tooltip but \
                 reported no surface for it, so the frame is structurally identical to one \
                 that never drew the tooltip. A backend that silently drops chrome must be \
                 detectable here.\n  expected surface id: {TOOLTIP_ID}\n  surfaces emitted: \
                 {emitted:?}",
            );
        }
    }

    /// #542 ask 1: the *set* of surfaces the two backends emit for the same
    /// frame must be the same set. This is the clause a backend that
    /// silently omits a border, a title or a scrollbar fails — its surface
    /// set is missing an entry the other backend has, and the diff names
    /// exactly which.
    ///
    /// The emptiness precondition is load-bearing: two backends that both
    /// report *nothing* have trivially equal surface sets, and a tier that
    /// passes vacuously is the state #542 is reporting, not a fix for it.
    #[test]
    fn structural_parity_backends_emit_the_same_tooltip_surface_set() {
        let [(a_name, a_inv), (b_name, b_inv)] = tooltip_frames();
        let (a, b) = (surfaces(&a_inv), surfaces(&b_inv));

        assert!(
            !a.is_empty() && !b.is_empty(),
            "STRUCTURAL PARITY (tier B) — contract §4b: the frame reported no surfaces at \
             all ({a_name}: {a:?}, {b_name}: {b:?}). Two empty sets are trivially equal, so \
             this clause must reject them explicitly — a vacuously-agreeing structural tier \
             is the blindness #542 reports, not parity."
        );

        assert_eq!(
            a,
            b,
            "STRUCTURAL PARITY (tier B) — #542 ask 1: the two backends emitted different \
             surface sets for the same frame. Missing from {b_name}: {:?}. Missing from \
             {a_name}: {:?}. A backend that silently drops chrome another backend draws \
             fails here — that is this clause's whole job (#542 acceptance bullet 1).",
            a.difference(&b).collect::<Vec<_>>(),
            b.difference(&a).collect::<Vec<_>>(),
        );
    }

    /// Contract §4b + §4c note 4 — the #541 clause.
    ///
    /// A tooltip surface must *enclose* its text on all four sides: the
    /// chrome (border, padding) occupies space above, below, left and
    /// right of the content. #541's TUI `draw_tooltip` painted `│` on the
    /// first and last column only — horizontal chrome, no vertical chrome
    /// — while GTK stroked a full box. A backend reporting a surface whose
    /// top or bottom edge is flush with its text has no vertical chrome,
    /// and fails here while tier A stays green.
    #[test]
    fn structural_parity_tooltip_surface_encloses_its_text_on_every_side() {
        for (name, inv) in tooltip_frames() {
            let zone = surface_bounds(&inv, TOOLTIP_ID).unwrap_or_else(|| {
                panic!(
                    "STRUCTURAL PARITY (tier B) precondition — contract §4b: {name} reported \
                     no {TOOLTIP_ID} surface, so there is no geometry to check. Fix \
                     `structural_parity_every_backend_reports_the_tooltip_surface` first.\n  \
                     surfaces emitted: {:?}",
                    surfaces(&inv)
                )
            });
            let text = text_bounds(&inv, TOOLTIP_TEXT).unwrap_or_else(|| {
                panic!(
                    "STRUCTURAL PARITY (tier B) precondition: {name} reported a \
                     {TOOLTIP_ID} surface but painted no {TOOLTIP_TEXT:?} run inside it."
                )
            });

            assert!(
                inv.inside(TOOLTIP_TEXT, &WidgetId::new(TOOLTIP_ID)),
                "STRUCTURAL PARITY (tier B) — contract §4b: on {name} the tooltip's text \
                 does not lie inside the surface the backend reported for it. surface \
                 {zone:?}, text {text:?}. A surface whose bounds do not contain what it \
                 drew is not an observation of the frame."
            );

            // Chrome on every side. Units are this backend's own (cells on
            // TUI, pixels on GTK) and are never compared across backends
            // here — only each backend's surface against its own text run.
            for (edge, has_chrome, gap) in [
                ("left", zone.x < text.x, text.x - zone.x),
                ("top", zone.y < text.y, text.y - zone.y),
                (
                    "right",
                    zone.x + zone.width > text.x + text.width,
                    (zone.x + zone.width) - (text.x + text.width),
                ),
                (
                    "bottom",
                    zone.y + zone.height > text.y + text.height,
                    (zone.y + zone.height) - (text.y + text.height),
                ),
            ] {
                assert!(
                    has_chrome,
                    "STRUCTURAL PARITY (tier B) — #541: {name}'s tooltip surface has no \
                     chrome on its {edge} edge (gap {gap}, in this backend's own units). \
                     GTK strokes a full 4-sided box; the pre-#541 TUI painted `│` on the \
                     first and last column only, so its box had no top or bottom — and \
                     `screen_has({TOOLTIP_TEXT:?})` was true on both. This is the clause \
                     that must catch that.\n  surface {zone:?}\n  text    {text:?}"
                );
            }
        }
    }

    /// #542 ask 4: "where the two backends *should* legitimately differ
    /// (pixel vs cell metrics), make that explicit in the assertion rather
    /// than absent from it."
    ///
    /// The tooltip surface is compared across backends as a **ratio to the
    /// text run it contains, measured in the same frame by the same
    /// backend** — so the cell/pixel scale factor divides out exactly and
    /// what remains is the shape of the chrome. A backend that registers a
    /// surface in the wrong space (pixel bounds recorded on a cell grid, a
    /// rect in `line_height` multiples left unresolved) lands orders of
    /// magnitude away and fails; sub-pixel differences in text measurement
    /// do not.
    ///
    /// TODO(test-author): contract §4 does not pin a tolerance for this
    /// comparison. `RATIO_TOLERANCE` is chosen coarse on purpose — it is a
    /// unit-confusion detector, not a rendering assertion. If the
    /// implementation needs it tighter or looser, that is a contract
    /// amendment, not a test edit.
    #[test]
    fn structural_parity_tooltip_surface_shape_agrees_across_unit_systems() {
        const RATIO_TOLERANCE: f32 = 0.35;

        let mut shapes: Vec<(&str, f32, f32)> = Vec::new();
        for (name, inv) in tooltip_frames() {
            let zone = surface_bounds(&inv, TOOLTIP_ID).unwrap_or_else(|| {
                panic!(
                    "STRUCTURAL PARITY (tier B) precondition — contract §4b: {name} reported \
                     no {TOOLTIP_ID} surface. surfaces emitted: {:?}",
                    surfaces(&inv)
                )
            });
            let text = text_bounds(&inv, TOOLTIP_TEXT).unwrap_or_else(|| {
                panic!("STRUCTURAL PARITY (tier B) precondition: {name} painted no {TOOLTIP_TEXT:?} run.")
            });
            assert!(
                text.width > 0.0 && text.height > 0.0,
                "STRUCTURAL PARITY (tier B) precondition: {name} reported a degenerate text \
                 run {text:?}, so it cannot serve as the unit-free denominator."
            );
            shapes.push((name, zone.width / text.width, zone.height / text.height));
        }

        let (a_name, a_w, a_h) = shapes[0];
        let (b_name, b_w, b_h) = shapes[1];

        for (axis, a_ratio, b_ratio) in [("width", a_w, b_w), ("height", a_h, b_h)] {
            let delta = (a_ratio - b_ratio).abs();
            let scale = a_ratio.abs().max(b_ratio.abs()).max(f32::EPSILON);
            assert!(
                delta / scale <= RATIO_TOLERANCE,
                "STRUCTURAL PARITY (tier B) — #542 ask 4: the tooltip surface has a \
                 different {axis} shape on each backend. Compared as surface-{axis} ÷ \
                 text-{axis} within one frame, so the cell-vs-pixel scale factor divides \
                 out and only the chrome's proportions remain.\n  {a_name}: {a_ratio}\n  \
                 {b_name}: {b_ratio}\n  relative difference {} exceeds tolerance \
                 {RATIO_TOLERANCE}",
                delta / scale
            );
        }
    }

    /// #542 ask 3 — the primitives with consumers today: status bar,
    /// sidebar chrome, the resize divider. Unlike the tooltip clauses this
    /// one is the **ratchet half**: `AppShell` already registers these
    /// regions on both backends, so it should be green today and stay
    /// green. It is here so that a #542 implementation which reworks the
    /// surface-recording path cannot quietly drop the chrome that is
    /// already observable.
    #[test]
    fn structural_parity_shell_chrome_surface_set_agrees_across_backends() {
        let [(a_name, a_inv), (b_name, b_inv)] = shell_frames();
        let (a, b) = (surfaces(&a_inv), surfaces(&b_inv));

        assert_eq!(
            a,
            b,
            "STRUCTURAL PARITY (tier B) — #542 ask 3: the two backends emitted different \
             shell-chrome surface sets for the same frame. Missing from {b_name}: {:?}. \
             Missing from {a_name}: {:?}.",
            a.difference(&b).collect::<Vec<_>>(),
            b.difference(&a).collect::<Vec<_>>(),
        );

        for required in [
            "app-shell:status-bar",
            "app-shell:sidebar-header",
            "app-shell:sidebar-content",
            "app-shell:divider",
            "app-shell:activity-bar",
        ] {
            assert!(
                a.contains(required),
                "STRUCTURAL PARITY (tier B) — #542 ask 3: neither backend reported the \
                 {required:?} chrome surface, so a backend that stopped drawing it could \
                 not be detected.\n  surfaces emitted: {a:?}"
            );
        }
    }

    // ── Contract gaps, recorded rather than guessed ───────────────────────
    //
    // TODO(test-author): contract §4 requires "both backends report a
    // tooltip zone", but does not say at what *granularity*. This slice
    // asserts one surface per primitive (the tooltip's own `WidgetId`)
    // plus set-equality across backends, which detects a whole primitive
    // going missing on one backend. It cannot, on its own, distinguish a
    // border sub-surface from padding inside a single reported box — the
    // enclosure clause above is the closest available proxy. If #542's
    // implementation chooses to emit per-chrome sub-surfaces (e.g. a
    // separate border surface), the set-equality clause covers it for
    // free; if it does not, no clause here demands one.
    //
    // TODO(test-author): #542 ask 3 also names the terminal divider (#533)
    // and settings chrome (#531). `app-shell:divider` is asserted above;
    // the *terminal* divider and the settings dialog's chrome are not
    // reachable from a fixture built on public API without restating those
    // issues' own layouts, and contract §4 does not specify an observable
    // for either. Left unasserted rather than invented.
    //
    // TODO(test-author): #542 acceptance bullet 2 — "re-running the suite
    // against the pre-#541 tree reproduces the tooltip divergence as a
    // failure" — is a *mutation check on a historical tree*, not a
    // property of one run, so it cannot be expressed as a test here.
    // Contract §4c note 4 records how to perform it (revert only
    // `draw_tooltip`'s border stroke, never the whole file) and it belongs
    // to the implementation's PR evidence, not to this slice.
}
