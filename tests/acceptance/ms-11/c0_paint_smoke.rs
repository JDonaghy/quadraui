// Sealed acceptance slice — ms-11 / issue #492 ("Tier C0: auto-generated
// per-primitive paint smoke + BackendCaps capability declaration").
//
// Authored independently from the implementation, against
// `tests/acceptance/ms-11/contract.md` §5. `include!`d from
// `quadraui/tests/acceptance.rs` below its SEALED marker.
//
// ── What this slice asserts, and what it deliberately does not ────────────
//
// Contract §5c is explicit that the *generator* is not the observable:
// "#492 auto-generates the per-primitive smoke; a slice that asserted the
// generated test *list* would restate the implementation and break on every
// new primitive. Assert the **properties above**, which hold whatever the
// generator emits."
//
// So this slice keeps its own small, hand-canned descriptor table (one
// exemplar per primitive, contract §6: inline, public quadraui API only)
// and asserts the two properties of contract §5b against it:
//
//   1. begin_frame → draw → end_frame does not panic, for every primitive
//      on every registered backend (#492 work item 1, first half);
//   2. the frame the backend produced is **observable** — a non-empty
//      `text_runs()` or a non-empty `zones()` (contract §5b), and for a
//      text-bearing descriptor, the text the descriptor asked for is
//      actually in `text_runs()` (#492 work item 1, second half).
//
// The descriptor table is *not* the generated list and makes no claim to
// be exhaustive; it is the fixture module #492 calls for ("one fixture
// module providing an exemplar of each primitive"), sampled. If the
// implementation's generator emits more primitives than are listed here,
// nothing in this slice breaks — that is the §5c property holding.
//
// ── Why "renders nothing" is invisible today ──────────────────────────────
//
// #492's premise is that `Backend`'s no-op defaults let a backend compile
// while discarding what it was handed — "Compiles must stop implying
// renders". The two backends this driver runs report a frame in different
// ways, and the difference is exactly where the blindness lives:
//
//   TuiDriver — `text_runs()` is a scan of the ratatui cell grid, so *any*
//               non-space glyph a primitive paints (a border, a scrollbar
//               thumb) shows up as a run.
//   GtkDriver — `text_runs()` comes from the `painted_text` map, i.e. only
//               real text draws. A GTK primitive that strokes rectangles
//               and paints no text contributes **nothing** to the frame
//               inventory, and `zones()` stays empty until a paint site
//               calls `Backend::register_zone` (quadraui#490 — today only
//               `AppShell::render` does).
//
// That is why the chrome-only descriptors below (scrollbar, terminal
// divider, drop overlay) are the sharp end of contract §5b: on a pixel
// backend they are, right now, indistinguishable from a primitive that was
// never drawn at all. Contract §5b's "non-empty `text_runs()` **or** a
// non-empty `zones()`" is the clause that makes that visible.
//
// ── Contract §5a is NOT asserted here. Read this before adding it. ────────
//
// Contract §5a's two Required clauses ("its declared `BackendCaps` is
// queryable from the conformance harness", "a skip … reports the reason
// string, and a skip is distinguishable from a pass") both name an API that
// does not exist yet: there is no `BackendCaps` type in quadraui's public
// surface today, and the capability list the conformance runner gates on is
// a private `&'static [&'static str]` inside the `conformance` *test*
// target, which this `acceptance` target cannot see.
//
// Naming `BackendCaps` from this slice would therefore not produce a red
// test — it would produce a **compile error in the shared `--test
// acceptance` target**, taking #554's and #542's slices down with it and
// reporting zero tests for every id in this milestone's manifest. Contract
// §6 rules that out in as many words ("a slice that fails to compile breaks
// the whole `--test acceptance` target — including other milestones'
// slices — rather than failing red on its own clause"), and it is the same
// stranded-on-a-harness-gap failure contract §4c refused to repeat for
// #542/#490.
//
// So §5a is left to a follow-up slice authored once #492's `BackendCaps`
// lands, exactly as §4c ordered #542 behind #490. See the
// `TODO(test-author)` block at the foot of this file for the two clauses
// that slice owes, written out so they are not lost.

#[cfg(all(feature = "tui", feature = "gtk"))]
mod ms11_492_c0_paint_smoke {
    use quadraui::gtk::testing::GtkDriver;
    use quadraui::testing::{ConformanceDriver, FrameInventory, LogicalViewport};
    use quadraui::tui::testing::TuiDriver;
    use quadraui::{
        compute_hunks, AppLogic, Backend, Color, CommandLine, Decoration, DiffEditability,
        DiffMode, DiffPane, DiffView, DropOverlay, ListItem, ListView, MessageList, MessageRow,
        PipelineStage, PipelineView, ProgressBar, Reaction, Rect, ScrollAxis, Scrollbar,
        SelectionMode, Spinner, StageStatus, StatusBar, StatusBarSegment, StyledSpan, StyledText,
        TabBar, TabItem, TextDisplay, TextDisplayLine, Tooltip, TooltipMeasure, TooltipPlacement,
        TreeRow, TreeStyle, TreeView, UiEvent, WidgetId,
    };

    /// Backend-neutral viewport. `LogicalViewport` is the unit-free
    /// constructor `ConformanceDriver` exposes precisely so a shared body
    /// never writes a cell or a pixel count (TUI reads it as cells, GTK
    /// scales it to pixels).
    ///
    /// Sized generously so no descriptor below is clipped for want of room
    /// — a C0 smoke that failed because its exemplar did not fit would be
    /// reporting the fixture, not the backend.
    const VIEWPORT: LogicalViewport = LogicalViewport::new(80, 24);

    // ── The canned descriptors (contract §6, #492 work item 1) ────────────
    //
    // One exemplar per primitive, built from public quadraui API only and
    // laid out in the backend's *own* metrics (`char_width` / `line_height`
    // / `viewport`), never in literal cells or pixels — `Rect::new(4.0,
    // 4.0, …)` would silently mean "4 cells" on TUI and a quarter of one
    // character on GTK.
    //
    // Every text-bearing descriptor carries a unique, space-free, 6-glyph
    // ASCII needle. Space-free because TUI reconstructs runs by breaking at
    // blank cells, so a needle containing a space can never match a TUI
    // run; short because several primitives budget their label against a
    // measured box and a long needle would be clipped for reasons that have
    // nothing to do with #492.

    /// One primitive's C0 case.
    struct PrimitiveCase {
        /// The `Backend` method this descriptor exercises. Used only in
        /// failure text, so a red row names the trait method to fix.
        method: &'static str,
        /// The text this descriptor hands the backend, or `None` for a
        /// chrome-only primitive that is asked to paint no text at all.
        needle: Option<&'static str>,
        /// Paints the descriptor into the frame. Receives the full
        /// viewport rect in the backend's own units.
        paint: fn(&mut dyn Backend, Rect),
    }

    fn id(suffix: &str) -> WidgetId {
        WidgetId::new(format!("ms11:492:{suffix}"))
    }

    const CASES: &[PrimitiveCase] = &[
        // ── Text-bearing primitives ──────────────────────────────────────
        PrimitiveCase {
            method: "draw_status_bar",
            needle: Some("c0stat"),
            paint: |b, area| {
                let lh = b.line_height();
                let bar = StatusBar {
                    id: id("status-bar"),
                    left_segments: vec![StatusBarSegment {
                        text: " c0stat ".to_string(),
                        fg: Color::rgb(220, 220, 220),
                        bg: Color::rgb(37, 37, 38),
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: vec![],
                };
                let _ = b.draw_status_bar(Rect::new(0.0, 0.0, area.width, lh), &bar, None, None);
            },
        },
        PrimitiveCase {
            method: "draw_tab_bar",
            needle: Some("c0tabs"),
            paint: |b, area| {
                let lh = b.line_height();
                let bar = TabBar {
                    id: id("tab-bar"),
                    tabs: vec![TabItem {
                        label: " c0tabs ".to_string(),
                        is_active: true,
                        is_dirty: false,
                        is_preview: false,
                        is_closable: false,
                    }],
                    scroll_offset: 0,
                    right_segments: vec![],
                    active_accent: None,
                    show_tab_close: false,
                    compact: false,
                };
                let _ = b.draw_tab_bar(Rect::new(0.0, 0.0, area.width, lh), &bar, None);
            },
        },
        PrimitiveCase {
            method: "draw_command_line",
            needle: Some("c0cmd"),
            paint: |b, area| {
                let lh = b.line_height();
                let cmd = CommandLine {
                    id: id("command-line"),
                    text: ":c0cmd".to_string(),
                    cursor_offset: None,
                    right_align: false,
                };
                b.draw_command_line(Rect::new(0.0, 0.0, area.width, lh), &cmd);
            },
        },
        PrimitiveCase {
            method: "draw_message_list",
            needle: Some("c0msg"),
            paint: |b, area| {
                let list = MessageList {
                    id: id("message-list"),
                    rows: vec![MessageRow::new("c0msg", Color::rgb(220, 220, 220), 0.0)],
                    scroll_top: 0,
                };
                b.draw_message_list(area, &list);
            },
        },
        PrimitiveCase {
            method: "draw_text_display",
            needle: Some("c0text"),
            paint: |b, area| {
                let td = TextDisplay {
                    id: id("text-display"),
                    lines: vec![TextDisplayLine {
                        spans: vec![StyledSpan::plain("c0text")],
                        decoration: Decoration::Normal,
                        timestamp: None,
                    }],
                    scroll_offset: 0,
                    auto_scroll: true,
                    max_lines: 0,
                    has_focus: false,
                    title: None,
                    show_scrollbar: false,
                };
                b.draw_text_display(area, &td);
            },
        },
        PrimitiveCase {
            method: "draw_list",
            needle: Some("c0list"),
            paint: |b, area| {
                let list = ListView {
                    id: id("list"),
                    title: None,
                    items: vec![ListItem {
                        text: StyledText::plain("c0list"),
                        icon: None,
                        detail: None,
                        decoration: Decoration::Normal,
                    }],
                    selected_idx: 0,
                    scroll_offset: 0,
                    has_focus: false,
                    bordered: false,
                    h_scroll: 0,
                    max_content_width: None,
                    show_v_scrollbar: false,
                };
                b.draw_list(area, &list);
            },
        },
        PrimitiveCase {
            method: "draw_tree",
            needle: Some("c0tree"),
            paint: |b, area| {
                let tree = TreeView {
                    id: id("tree"),
                    rows: vec![TreeRow {
                        path: vec![0],
                        indent: 0,
                        icon: None,
                        text: StyledText::plain("c0tree"),
                        badge: None,
                        is_expanded: None,
                        decoration: Decoration::Normal,
                        edit: None,
                    }],
                    selection_mode: SelectionMode::Single,
                    selected_path: None,
                    scroll_offset: 0,
                    style: TreeStyle::default(),
                    has_focus: false,
                };
                b.draw_tree(area, &tree);
            },
        },
        PrimitiveCase {
            method: "draw_pipeline_view",
            needle: Some("c0pipe"),
            paint: |b, area| {
                let view = PipelineView {
                    id: id("pipeline"),
                    stages: vec![PipelineStage {
                        label: "c0pipe".to_string(),
                        status: StageStatus::Active,
                        action: None,
                    }],
                    focused_stage: None,
                };
                let _ = b.draw_pipeline_view(area, &view);
            },
        },
        PrimitiveCase {
            method: "draw_diff_view",
            needle: Some("c0diff"),
            paint: |b, area| {
                // Named by #492 itself: "including the current macOS
                // `draw_diff_view` fake". The needle lives in the *row
                // content*, not in a pane label, so a backend that paints
                // the chrome and drops the diff body still fails.
                let left = "c0diff-old".to_string();
                let right = "c0diff-new".to_string();
                let hunks = compute_hunks(&left, &right);
                let view = DiffView {
                    id: id("diff-view"),
                    left,
                    right,
                    left_label: None,
                    right_label: None,
                    hunks,
                    mode: DiffMode::Unified,
                    editability: DiffEditability::ReadOnly,
                    scroll_offset: 0,
                    focused_pane: DiffPane::Left,
                    has_focus: false,
                };
                let _ = b.draw_diff_view(area, &view);
            },
        },
        PrimitiveCase {
            method: "draw_tooltip",
            needle: Some("c0tip"),
            paint: |b, area| {
                let cw = b.char_width();
                let lh = b.line_height();
                let anchor = Rect::new(0.0, 0.0, area.width, lh);
                let tooltip = Tooltip {
                    id: id("tooltip"),
                    text: "c0tip".to_string(),
                    styled_lines: None,
                    placement: TooltipPlacement::Bottom,
                    bg: None,
                    fg: None,
                };
                // Room for a border on all four sides plus the whole label
                // — a box measured to exactly `border + text + border`
                // leaves a backend no padding and clips the last glyph.
                let measure = TooltipMeasure::new(cw * 9.0, lh * 3.0);
                let layout = tooltip.layout(anchor, area, measure, lh);
                b.draw_tooltip(&tooltip, &layout);
            },
        },
        PrimitiveCase {
            method: "draw_spinner",
            needle: Some("c0spin"),
            paint: |b, area| {
                let cw = b.char_width();
                let lh = b.line_height();
                let spinner = Spinner {
                    id: id("spinner"),
                    label: "c0spin".to_string(),
                    frame_idx: 0,
                    accent: None,
                };
                let _ = b.draw_spinner(Rect::new(0.0, 0.0, cw * 20.0, lh), &spinner);
                let _ = area;
            },
        },
        PrimitiveCase {
            method: "draw_progress",
            needle: Some("c0prog"),
            paint: |b, area| {
                let cw = b.char_width();
                let lh = b.line_height();
                let bar = ProgressBar {
                    id: id("progress"),
                    label: "c0prog".to_string(),
                    value: Some(0.5),
                    frame_idx: 0,
                    cancellable: false,
                    accent: None,
                };
                let _ = b.draw_progress(Rect::new(0.0, 0.0, cw * 30.0, lh * 2.0), &bar);
                let _ = area;
            },
        },
        // ── Chrome-only primitives (contract §5b's sharp end) ─────────────
        //
        // These descriptors hand the backend NO text. On a pixel backend
        // their entire output is strokes and fills, so the only way the
        // frame can report them at all is a registered zone. A C0 tier that
        // cannot see them is a C0 tier that cannot tell "drew the
        // scrollbar" from "took the no-op default".
        PrimitiveCase {
            method: "draw_scrollbar",
            needle: None,
            paint: |b, area| {
                let cw = b.char_width();
                let track = Rect::new(area.width - cw, 0.0, cw, area.height);
                let sb = Scrollbar {
                    id: id("scrollbar"),
                    axis: ScrollAxis::Vertical,
                    track,
                    thumb_start: 0.0,
                    thumb_len: area.height / 2.0,
                    hovered: false,
                    dragging: false,
                };
                b.draw_scrollbar(track, &sb);
            },
        },
        PrimitiveCase {
            method: "draw_terminal_divider",
            needle: None,
            paint: |b, area| {
                let lh = b.line_height();
                b.draw_terminal_divider(Rect::new(0.0, lh * 2.0, area.width, lh));
            },
        },
        PrimitiveCase {
            method: "draw_drop_overlay",
            needle: None,
            paint: |b, area| {
                let cw = b.char_width();
                let lh = b.line_height();
                let overlay = DropOverlay {
                    highlight: Some(Rect::new(cw * 2.0, lh * 2.0, cw * 20.0, lh * 6.0)),
                    insertion_bar: None,
                    ghost_position: None,
                };
                b.draw_drop_overlay(&overlay);
                let _ = area;
            },
        },
    ];

    // ── Fixture + observation plumbing ───────────────────────────────────

    /// An `AppLogic` whose whole render pass is one primitive's canned
    /// descriptor — the minimal "draw this once into a fixture" frame
    /// contract §5b calls for.
    ///
    /// Defined inline, public quadraui API only, per contract §6: a fixture
    /// needing a new constructor on an `examples/common` type could not
    /// compile until that constructor landed, and a slice that fails to
    /// compile takes every other milestone's slice down with it instead of
    /// failing red on its own clause.
    struct PrimitiveFixture {
        paint: fn(&mut dyn Backend, Rect),
    }

    impl AppLogic for PrimitiveFixture {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let vp = backend.viewport();
            (self.paint)(backend, Rect::new(0.0, 0.0, vp.width, vp.height));
        }

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    /// Paint one frame of `app` on backend `D` and return its inventory.
    ///
    /// Generic over `ConformanceDriver` rather than duplicated per backend:
    /// a shared body cannot accidentally assert something on one backend it
    /// forgot to assert on the other, which is the honesty gap #492 is
    /// about.
    ///
    /// Constructing the driver *is* the begin_frame → draw → end_frame
    /// round trip, so a primitive that panics mid-paint unwinds out of
    /// here and fails whichever clause called it.
    fn frame<D, A>(app: A) -> FrameInventory
    where
        A: AppLogic,
        D: ConformanceDriver<App = A>,
    {
        D::new_fixture(app, VIEWPORT).inventory()
    }

    /// Both registered backends' inventories for one case, in a fixed order
    /// so every message below names them the same way.
    fn frames(case: &PrimitiveCase) -> [(&'static str, FrameInventory); 2] {
        [
            (
                "TuiDriver",
                frame::<TuiDriver<_>, _>(PrimitiveFixture { paint: case.paint }),
            ),
            (
                "GtkDriver",
                frame::<GtkDriver<_>, _>(PrimitiveFixture { paint: case.paint }),
            ),
        ]
    }

    /// A compact dump of what the frame *did* report, for failure text.
    fn reported(inv: &FrameInventory) -> String {
        let runs: Vec<&str> = inv
            .text_runs()
            .iter()
            .map(|r| r.text.as_str())
            .filter(|t| !t.trim().is_empty())
            .take(20)
            .collect();
        let zones: Vec<&str> = inv.zones().iter().map(|z| z.id.as_str()).collect();
        format!("text_runs (first 20): {runs:?}; zones: {zones:?}")
    }

    /// Fail with **every** gap the clause found, not just the first.
    ///
    /// #492's second acceptance bullet is "Win/macOS gaps enumerate as
    /// red/skip rows, **not silence**", and the same reasoning applies
    /// within a single clause: a bare `assert!` inside the loop stops at
    /// primitive #1, so the implementer fixes one method, re-runs, and
    /// discovers the next — a serialised checklist instead of a matrix.
    /// Each clause below therefore collects its rows and reports them
    /// together, so one red run enumerates the whole gap.
    fn report_gaps(clause: &str, gaps: Vec<String>) {
        if !gaps.is_empty() {
            panic!(
                "{clause}\n\n{} gap(s), one row per (backend, primitive):\n  {}",
                gaps.len(),
                gaps.join("\n  ")
            );
        }
    }

    // ═══ C0 clause 1 — the frame completes ═══════════════════════════════
    //
    // #492 work item 1, first half: "begin_frame → draw → end_frame must
    // not panic". `frames()` performs exactly that round trip on both
    // backends, so a panicking primitive fails here by unwinding.
    //
    // Expected GREEN today — this is the boot clause, and a red row here
    // means a primitive is not merely invisible but actively crashing.

    #[test]
    fn c0_paint_smoke_every_primitive_survives_a_frame_on_every_backend() {
        assert!(
            !CASES.is_empty(),
            "C0 (contract §5b): the descriptor table is empty, so every clause in this slice \
             would pass vacuously. A C0 tier with no primitives in it is not a tier."
        );

        let mut painted = 0usize;
        for case in CASES {
            for (name, _inv) in frames(case) {
                // Reaching here means begin_frame → draw → end_frame
                // completed on `name` without unwinding.
                let _ = name;
                painted += 1;
            }
        }

        assert_eq!(
            painted,
            CASES.len() * 2,
            "C0 (contract §5b): expected every one of the {} descriptors to be painted on both \
             registered backends, but only {painted} frames completed.",
            CASES.len()
        );
    }

    // ═══ C0 clause 2 — text-bearing primitives report their text ═════════
    //
    // #492 work item 1, second half: "for text-bearing primitives,
    // `FrameInventory::text_runs()` must be non-empty". Stated one notch
    // sharper here, because "non-empty" alone is satisfiable by a backend
    // that paints its own chrome and drops the caller's content — which is
    // exactly the macOS `draw_diff_view` fake #492 names. The needle the
    // descriptor handed the backend must come back.

    #[test]
    fn c0_paint_smoke_text_bearing_primitives_report_the_text_they_were_given() {
        let mut gaps = Vec::new();
        for case in CASES {
            let Some(needle) = case.needle else {
                continue;
            };
            for (name, inv) in frames(case) {
                if !inv.screen_has(needle) {
                    gaps.push(format!(
                        "{name}::{} was handed {needle:?} and the frame does not report it — {}",
                        case.method,
                        reported(&inv)
                    ));
                }
            }
        }
        report_gaps(
            "C0 (contract §5b, #492 work item 1): a text-bearing primitive whose text never \
             reaches `text_runs()` is the \"compiles but renders nothing\" case this tier exists \
             to catch — the same shape as the macOS `draw_diff_view` fake.",
            gaps,
        );
    }

    // ═══ C0 clause 3 — every primitive is observable at all ══════════════
    //
    // Contract §5b, verbatim: "every primitive the backend declares support
    // for produces a **non-empty** `inventory().text_runs()` **or** a
    // non-empty `zones()` when drawn once into a fixture."
    //
    // RED TODAY for the chrome-only descriptors on the pixel backend. A
    // GTK scrollbar / terminal divider / drop overlay paints no text, and
    // no paint site outside `AppShell::render` calls
    // `Backend::register_zone` (quadraui#490), so the frame that drew it is
    // byte-identical to the frame that took the trait's no-op default.
    // That indistinguishability is the whole premise of #492.
    //
    // Deliberately weak, per contract §5b: this is a boot tier ("this
    // primitive draws *something*"), not a rendering assertion.
    // Strengthening it belongs to C1/#491.

    #[test]
    fn c0_paint_smoke_every_primitive_is_observable_in_the_frame_inventory() {
        let mut gaps = Vec::new();
        for case in CASES {
            for (name, inv) in frames(case) {
                if inv.text_runs().is_empty() && inv.zones().is_empty() {
                    gaps.push(format!(
                        "{name}::{} reported neither a text run nor a zone — {}",
                        case.method,
                        reported(&inv)
                    ));
                }
            }
        }
        report_gaps(
            "C0 (contract §5b): a primitive was drawn and the frame reports nothing at all. Such \
             a frame is indistinguishable from one in which the method took its no-op default \
             and painted nothing, which is precisely what #492 exists to make impossible \
             (\"compiles must stop implying renders\"). A chrome-only primitive has no text to \
             report, so the observable it owes the inventory is a `Backend::register_zone` call \
             at its paint site.",
            gaps,
        );
    }

    // ═══ C0 clause 4 — the chrome-only primitives are attributable ═══════
    //
    // Clause 3 is satisfiable on a cell backend by accident: TUI's
    // `text_runs()` is a scan of the character grid, so a box-drawing glyph
    // a scrollbar happens to paint reads as a "text run" and the frame
    // looks observable even though nothing named the scrollbar. Clause 4
    // asks the stronger, unit-free question for the descriptors that carry
    // no text: does the frame say *which* primitive it drew?
    //
    // RED TODAY on both backends. This is the clause a per-primitive C0
    // tier needs in order to report a per-primitive verdict rather than a
    // per-frame one.
    //
    // TODO(test-author): contract §5 does not pin the zone id a primitive
    // must register. This clause therefore asserts only that *some* zone is
    // registered for a chrome-only descriptor, not what it is called — the
    // strongest claim derivable from §5b without inventing a naming
    // convention the implementation has not chosen. (#542's slice asserts
    // id-level agreement for the primitives that already register zones;
    // extending that to these belongs with whatever convention #492 picks.)

    #[test]
    fn c0_paint_smoke_chrome_only_primitives_register_a_zone() {
        let chrome: Vec<&PrimitiveCase> = CASES.iter().filter(|c| c.needle.is_none()).collect();
        assert!(
            !chrome.is_empty(),
            "C0: the descriptor table lists no chrome-only primitive, so this clause would \
             pass vacuously. The text-free primitives are the ones a text-only observer \
             cannot see at all."
        );

        let mut gaps = Vec::new();
        for case in chrome {
            for (name, inv) in frames(case) {
                if inv.zones().is_empty() {
                    gaps.push(format!(
                        "{name}::{} registered no zone — {}",
                        case.method,
                        reported(&inv)
                    ));
                }
            }
        }
        report_gaps(
            "C0 (contract §5b): these primitives paint no text by construction, and the frame \
             registered no zone for them either — so nothing in the inventory attributes any \
             part of the frame to the primitive that was drawn. A backend that silently dropped \
             the call would produce the identical inventory. (On a cell backend the previous \
             clause can pass by accident here: TUI's `text_runs()` is a scan of the character \
             grid, so a box-drawing glyph reads as a \"text run\" even though nothing named the \
             primitive.)",
            gaps,
        );
    }

    // ── Contract gaps and deferred clauses, recorded rather than guessed ──
    //
    // TODO(test-author): **contract §5a is unasserted by this slice, and
    // owes a follow-up slice.** Both of its Required clauses name API that
    // does not exist yet:
    //
    //   §5a.1 "for every registered backend, its declared `BackendCaps` is
    //         queryable from the conformance harness";
    //   §5a.2 "a scenario skipped for capability reasons reports the
    //         **reason string**, and a skip is distinguishable from a pass
    //         in the runner's output" — §5a calls this the load-bearing
    //         one, because a capability system whose skip reads as a pass
    //         turns every unimplemented backend into a green matrix.
    //
    // There is no `BackendCaps` in quadraui's public API today, and the
    // capability list the runner gates on lives in the *`conformance`* test
    // target, invisible from this *`acceptance`* target. Writing either
    // clause now yields a compile error, not a red test — and a compile
    // error in this shared target reports ZERO tests for #554's and #542's
    // slices too (contract §6). Contract §4c already established the right
    // answer for this exact situation ("author the slice after the
    // dependency merges" rather than stranding it on a harness gap); this
    // slice follows it. The follow-up slice should assert §5a.1 by querying
    // each registered backend's caps, and §5a.2 by driving a scenario whose
    // `requires` names a capability the backend does not declare and
    // checking that the resulting outcome (a) is not a pass and (b) carries
    // the missing capability's name.
    //
    // TODO(test-author): #492's third work item, "wire the C0 suite into
    // the conformance runner (B4) as tier 0", is an integration fact about
    // `tests/conformance.rs` — a different cargo test target, with its own
    // matrix output. It is not observable from this target and contract §5
    // specifies no observable for it, so it is left to that suite's own
    // tier-0 rows rather than restated here.
    //
    // TODO(test-author): #492's second acceptance bullet, "Win/macOS gaps
    // enumerate as red/skip rows, not silence", cannot be asserted from
    // this fleet at all: contract §7.2 records that no macOS runner exists,
    // and no Win driver is registered with this acceptance target. The
    // clause is real but unrunnable here — per §7.2, an unrunnable suite
    // reads as coverage and is worse than none.
    //
    // TODO(test-author): #492's first acceptance bullet is a **mutation
    // check** ("C0 red on a backend whose draw method paints nothing for a
    // text-bearing primitive — prove by temporarily blanking one"). That is
    // a property of a deliberately-mutated tree, not of one run, so it
    // cannot be expressed as a test here.
    // `c0_paint_smoke_text_bearing_primitives_report_the_text_they_were_given`
    // is the clause that fires under that mutation; performing the mutation
    // and pasting its red output belongs to the implementation's PR
    // evidence.
}
