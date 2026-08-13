//! Tier C0 — auto-generated per-primitive paint smoke (quadraui#492,
//! epic #480 acceptance: "Compiles must stop implying renders").
//!
//! `Backend` gives several `draw_*`-adjacent methods a no-op (or `false`)
//! default so a new backend compiles before every optional surface is
//! wired up. That silence is the bug: a backend can implement every
//! `draw_*` method and still discard the caller's content (the macOS
//! `draw_diff_view` fake #492 names), and nothing in a green build says
//! so. C0 is the boot tier that closes that gap: for every primitive in
//! [`CASES`], `begin_frame` → draw → `end_frame` must not panic, and the
//! resulting frame must be **observable** — a non-empty
//! `FrameInventory::text_runs()` or a non-empty `zones()` — on every
//! registered backend.
//!
//! [`CASES`] *is* the generator: [`run`] is generic over
//! [`super::runner::DriverFactory`], so adding a backend costs nothing
//! here (same "one factory, no per-backend code" shape as
//! `fixtures::build`), and adding a primitive is one more entry in the
//! table rather than one more hand-written test function per backend.
//!
//! Deliberately weak, matching contract §5b (`tests/acceptance/ms-11/
//! contract.md`): this is "the primitive draws *something* attributable
//! to it", not a rendering assertion — C1/#491's scenario suite is where
//! layout and interaction get checked.

use quadraui::{
    compute_hunks, AppLogic, Backend, Color, CommandLine, Decoration, DiffEditability, DiffMode,
    DiffPane, DiffView, DropOverlay, ListItem, ListView, MessageList, MessageRow, PipelineStage,
    PipelineView, ProgressBar, Reaction, Rect, ScrollAxis, Scrollbar, SelectionMode, Spinner,
    StageStatus, StatusBar, StatusBarSegment, StyledSpan, StyledText, TabBar, TabItem, TextDisplay,
    TextDisplayLine, Tooltip, TooltipMeasure, TooltipPlacement, TreeRow, TreeStyle, TreeView,
    UiEvent, WidgetId,
};

use super::runner::{DriverFactory, DynDriver};

/// Backend-neutral viewport, generous enough that no descriptor below
/// clips for want of room — a C0 smoke that failed on fixture size would
/// be reporting the fixture, not the backend.
const VIEWPORT: quadraui::testing::LogicalViewport =
    quadraui::testing::LogicalViewport::new(80, 24);

fn id(suffix: &str) -> WidgetId {
    WidgetId::new(format!("c0:{suffix}"))
}

/// One primitive's C0 case.
pub struct Case {
    /// The `Backend` method this case exercises — named in failure text
    /// so a red row points straight at the trait method to fix.
    pub method: &'static str,
    /// The text this descriptor hands the backend, or `None` for a
    /// chrome-only primitive that paints no text at all.
    pub needle: Option<&'static str>,
    /// Paints the descriptor into the given viewport rect, in the
    /// backend's own units (never literal cells or pixels).
    pub paint: fn(&mut dyn Backend, Rect),
}

/// One exemplar per primitive, built from public quadraui API only.
/// Sampled, not exhaustive — extending coverage is one more entry.
pub const CASES: &[Case] = &[
    Case {
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
    Case {
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
    Case {
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
    Case {
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
    Case {
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
    Case {
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
    Case {
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
    Case {
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
    Case {
        method: "draw_diff_view",
        needle: Some("c0diff"),
        paint: |b, area| {
            // The needle lives in the row content, not a pane label, so a
            // backend that paints the chrome and drops the diff body
            // (the macOS fake #492 names) still fails.
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
    Case {
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
            // Room for a border on all sides plus the whole label — a box
            // measured to exactly `border + text + border` leaves no
            // padding and clips the last glyph.
            let measure = TooltipMeasure::new(cw * 9.0, lh * 3.0);
            let layout = tooltip.layout(anchor, area, measure, lh);
            b.draw_tooltip(&tooltip, &layout);
        },
    },
    Case {
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
    Case {
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
    // ── Chrome-only primitives — no text, so the only way the frame can
    // report them at all is a registered zone (contract §5b's sharp end).
    Case {
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
    Case {
        method: "draw_terminal_divider",
        needle: None,
        paint: |b, area| {
            let lh = b.line_height();
            b.draw_terminal_divider(Rect::new(0.0, lh * 2.0, area.width, lh));
        },
    },
    Case {
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

/// An `AppLogic` whose whole render pass is one primitive's canned
/// descriptor.
struct Fixture {
    paint: fn(&mut dyn Backend, Rect),
}

impl AppLogic for Fixture {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        (self.paint)(backend, Rect::new(0.0, 0.0, vp.width, vp.height));
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

/// One case's outcome on one backend. Positional: `run()` returns exactly
/// one `CaseOutcome` per [`CASES`] entry, in the same order, so callers
/// zip the two rather than this type repeating `Case::method`.
pub struct CaseOutcome {
    /// `false` means the primitive panicked mid-paint — a worse failure
    /// than painting nothing, and the one C0's boot half exists to catch.
    pub survived: bool,
    /// `true` when the descriptor had no needle, or the needle it handed
    /// the backend came back in `text_runs()`.
    pub text_ok: bool,
    /// `true` when the frame reports a non-empty `text_runs()` or a
    /// non-empty `zones()` — contract §5b's observability floor.
    pub observable: bool,
    /// A compact dump of what the frame reported, for failure text.
    pub reported: String,
}

/// Run every [`CASES`] entry against one backend, built via `F`.
///
/// Generic over [`DriverFactory`] rather than duplicated per backend, so
/// a shared body can't accidentally assert something on one backend it
/// forgot to check on another.
pub fn run<F: DriverFactory>() -> Vec<CaseOutcome> {
    CASES
        .iter()
        .map(|case| {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let driver: Box<dyn DynDriver> = F::make(Fixture { paint: case.paint }, VIEWPORT);
                let inv = driver.inventory();
                let text_ok = match case.needle {
                    Some(needle) => driver.screen_has(needle),
                    None => true,
                };
                let observable = !inv.text_runs().is_empty() || !inv.zones().is_empty();
                let runs: Vec<String> = inv
                    .text_runs()
                    .iter()
                    .map(|r| r.text.clone())
                    .filter(|t| !t.trim().is_empty())
                    .take(10)
                    .collect();
                let zones: Vec<String> = inv
                    .zones()
                    .iter()
                    .map(|z| z.id.as_str().to_string())
                    .collect();
                (
                    text_ok,
                    observable,
                    format!("text_runs: {runs:?}; zones: {zones:?}"),
                )
            }));
            match outcome {
                Ok((text_ok, observable, reported)) => CaseOutcome {
                    survived: true,
                    text_ok,
                    observable,
                    reported,
                },
                Err(_) => CaseOutcome {
                    survived: false,
                    text_ok: false,
                    observable: false,
                    reported: "panicked mid-paint".to_string(),
                },
            }
        })
        .collect()
}
