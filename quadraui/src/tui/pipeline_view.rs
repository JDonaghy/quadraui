//! TUI entry point for [`crate::primitives::pipeline_view::PipelineView`];
//! the cell painter is the widget's own
//! (`primitives/pipeline_view/paint_cells.rs`).
//!
//! Paints a horizontal row of rounded stage boxes connected by `───▶`
//! arrow connectors. Corner glyphs `╭ ╮ ╰ ╯` give the boxes soft rounded
//! corners, matching the `border-radius` used by the GTK rasteriser. Each
//! box shows a status icon on the first row, the stage label on as many
//! rows as fit directly below it, bounded by the box border and the
//! action button row. See [`label_row_bounds`] for how label rows are
//! computed from the rows actually painted. An optional `[Action]` button
//! appears on the bottom row.
//!
//! ## Colour mapping
//!
//! | Status   | Icon | Colour                        |
//! |----------|------|-------------------------------|
//! | Done     | ✓    | green (`theme.success`)        |
//! | Active   | ●    | yellow (`theme.accent_bg`)     |
//! | Failed   | ✗    | red (`theme.error`)            |
//! | Pending  | ·    | dim (`theme.muted_fg`)         |
//! | Skipped  | ─    | grey (`theme.muted_fg`)        |
//!
//! The focused stage (keyboard navigation) shows a `▼` caret in the row
//! reserved above the box; the box border retains its per-status colour.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::BufferCellSurface;
#[cfg(test)]
use crate::primitives::pipeline_view::paint_cells::label_row_bounds;
use crate::primitives::pipeline_view::{PipelineView, PipelineViewLayout};
use crate::theme::Theme;

/// Rows reserved above the stage boxes for the focus caret.
#[cfg(test)]
const TUI_FOCUS_INDICATOR_H: u16 =
    crate::primitives::pipeline_view::PIPELINE_FOCUS_INDICATOR_CELLS as u16;

/// Compute the TUI cell-unit layout for a [`PipelineView`] without painting
/// — [`PipelineView::cell_layout`].
pub fn tui_pipeline_view_layout(view: &PipelineView, area: Rect) -> PipelineViewLayout {
    view.cell_layout(crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    ))
}

/// Paints `view` into `area` and returns the layout for host click
/// dispatch.
pub fn draw_pipeline_view(
    buf: &mut Buffer,
    area: Rect,
    view: &PipelineView,
    theme: &Theme,
) -> PipelineViewLayout {
    let layout = tui_pipeline_view_layout(view, area);
    crate::primitives::pipeline_view::paint_cells::paint(
        &mut BufferCellSurface::new(buf),
        view,
        &layout,
        theme,
    );
    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::pipeline_view::{PipelineHit, PipelineStage, StageStatus};
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn make_view() -> PipelineView {
        PipelineView {
            id: WidgetId::new("pipe"),
            stages: vec![
                PipelineStage {
                    label: "Build".into(),
                    status: StageStatus::Done,
                    action: None,
                },
                PipelineStage {
                    label: "Test".into(),
                    status: StageStatus::Active,
                    action: Some("Retry".into()),
                },
            ],
            focused_stage: None,
        }
    }

    #[test]
    fn draws_without_panic_and_has_borders() {
        let area = Rect::new(0, 0, 30, 5);
        let mut buf = Buffer::empty(area);
        let view = make_view();
        draw_pipeline_view(&mut buf, area, &view, &Theme::default());
        // Row 0 is reserved for the focus indicator; box starts at row 1.
        // Rounded-corner glyph ╭ is the top-left corner.
        assert_eq!(cell_char(&buf, 0, 1), '╭');
    }

    /// Shared body for the action↔click round trip, run at both the
    /// origin and a non-zero `area` origin. `tui_pipeline_view_layout`
    /// bakes `area.x`/`area.y` straight into the returned bounds (absolute
    /// frame, matching the GTK/macOS twins) and adds `TUI_FOCUS_INDICATOR_H`
    /// to `area.y` before laying out. Deriving `ab`/`bb` from the layout
    /// ensures this exercises the actual origin math the function performs,
    /// at any origin.
    fn layout_hit_test_action_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 40, 5);
        let mut buf = Buffer::empty(area);
        let view = make_view();
        let layout = draw_pipeline_view(&mut buf, area, &view, &Theme::default());

        // Box top must sit exactly one row below area.y + the reserved
        // focus-indicator row, not at a hardcoded absolute row — pins the
        // offset math independently of the hit_test round trip below.
        let bb0 = layout.stages[0].box_bounds;
        assert_eq!(
            bb0.y,
            (area.y + TUI_FOCUS_INDICATOR_H) as f32,
            "stage box top should be area.y + TUI_FOCUS_INDICATOR_H"
        );

        // Stage 1 has action bounds.
        let ab = layout.stages[1]
            .action_bounds
            .expect("action bounds for stage 1");
        let hit = layout.hit_test(ab.x + ab.width / 2.0, ab.y + ab.height / 2.0);
        assert_eq!(hit, PipelineHit::Action(1));
    }

    #[test]
    fn layout_hit_test_action_round_trip() {
        layout_hit_test_action_round_trip_at(0, 0);
    }

    /// Non-zero-origin regression guard.
    #[test]
    fn layout_hit_test_action_round_trip_at_nonzero_origin() {
        layout_hit_test_action_round_trip_at(7, 13);
    }

    fn layout_hit_test_body_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 40, 5);
        let mut buf = Buffer::empty(area);
        let view = make_view();
        let layout = draw_pipeline_view(&mut buf, area, &view, &Theme::default());

        let bb = layout.stages[0].box_bounds;
        assert_eq!(
            bb.y,
            (area.y + TUI_FOCUS_INDICATOR_H) as f32,
            "stage box top should be area.y + TUI_FOCUS_INDICATOR_H"
        );
        let hit = layout.hit_test(bb.x + 1.0, bb.y + 1.0);
        assert_eq!(hit, PipelineHit::Body(0));
    }

    #[test]
    fn layout_hit_test_body_round_trip() {
        layout_hit_test_body_round_trip_at(0, 0);
    }

    /// Non-zero-origin regression guard.
    #[test]
    fn layout_hit_test_body_round_trip_at_nonzero_origin() {
        layout_hit_test_body_round_trip_at(7, 13);
    }

    #[test]
    fn zero_size_area_is_noop() {
        let buf_area = Rect::new(0, 0, 10, 5);
        let mut buf = Buffer::empty(buf_area);
        let area = Rect::new(0, 0, 0, 0);
        let view = make_view();
        let _layout = draw_pipeline_view(&mut buf, area, &view, &Theme::default());
        // Buffer should remain empty.
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    /// Focused Done stage: ▼ appears in row 0 and the box border is still drawn.
    #[test]
    fn focused_done_stage_shows_indicator_and_retains_border() {
        let area = Rect::new(0, 0, 30, 6);
        let mut buf = Buffer::empty(area);
        let mut view = make_view();
        view.focused_stage = Some(0); // stage 0 is Done
        let layout = draw_pipeline_view(&mut buf, area, &view, &Theme::default());

        let bb = layout.stages[0].box_bounds;
        let bx = bb.x.round() as u16;
        let by = bb.y.round() as u16;
        let bw = bb.width.round() as u16;

        // Box must be pushed down by the indicator row.
        assert_eq!(by, 1, "box top must be at indicator row + 1");

        // The ▼ caret appears in the reserved row above the box.
        let ind_col = bx + bw / 2;
        assert_eq!(cell_char(&buf, ind_col, 0), '▼');

        // The box border is still drawn (rounded, status colour, not overridden).
        assert_eq!(cell_char(&buf, bx, by), '╭');
    }

    /// A label carrying a newline renders as two centred rows inside the box —
    /// no literal newline cell on row 1, second line on its own row. Regression
    /// guard for the off-box "black gap" glitch.
    #[test]
    fn two_line_label_renders_on_separate_rows() {
        let area = Rect::new(0, 0, 40, 8);
        let mut buf = Buffer::empty(area);
        let view = PipelineView {
            id: WidgetId::new("pipe"),
            stages: vec![PipelineStage {
                label: "Stage One\n3:45".into(),
                status: StageStatus::Active,
                action: None,
            }],
            focused_stage: None,
        };
        let layout = draw_pipeline_view(&mut buf, area, &view, &Theme::default());

        let bb = layout.stages[0].box_bounds;
        let by = bb.y.round() as u16;
        let bh = bb.height.round() as u16;
        let (first_row, _) = label_row_bounds(by, bh, layout.stages[0].action_bounds);

        // Label must start immediately below the icon's actual rendered row
        // (icon_row = by + 1) — no blank gap row, regardless of
        // `label_bounds`'s proportional icon-height assumption.
        assert_eq!(
            first_row,
            by + 2,
            "label should start directly below the icon row, with no gap"
        );

        // Collect the rendered glyphs of the two label rows.
        let row1: String = (0..area.width)
            .map(|x| cell_char(&buf, x, first_row))
            .collect();
        let row2: String = (0..area.width)
            .map(|x| cell_char(&buf, x, first_row + 1))
            .collect();

        // Line 1 holds the stage name; line 2 holds the elapsed time.
        assert!(row1.contains("Stage One"), "row1 = {row1:?}");
        assert!(row2.contains("3:45"), "row2 = {row2:?}");
        // The elapsed segment must NOT bleed onto row 1 (the old glitch).
        assert!(!row1.contains("3:45"), "elapsed leaked onto row1: {row1:?}");
        // No literal newline rendered as a cell anywhere on row 1.
        assert!(!row1.contains('\n'), "newline rendered as a cell: {row1:?}");
    }

    /// A 3-line label renders all three lines on their own rows when the box
    /// has room, ensuring arbitrary N-line labels are fully displayed.
    #[test]
    fn three_line_label_renders_on_separate_rows() {
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let view = PipelineView {
            id: WidgetId::new("pipe"),
            stages: vec![PipelineStage {
                label: "Line One\nLine Two\nLine Three".into(),
                status: StageStatus::Active,
                action: None,
            }],
            focused_stage: None,
        };
        let layout = draw_pipeline_view(&mut buf, area, &view, &Theme::default());

        let bb = layout.stages[0].box_bounds;
        let by = bb.y.round() as u16;
        let bh = bb.height.round() as u16;
        let (first_row, _) = label_row_bounds(by, bh, layout.stages[0].action_bounds);

        let row_text =
            |row: u16| -> String { (0..area.width).map(|x| cell_char(&buf, x, row)).collect() };

        assert!(
            row_text(first_row).contains("Line One"),
            "row {first_row} = {:?}",
            row_text(first_row)
        );
        assert!(
            row_text(first_row + 1).contains("Line Two"),
            "row {} = {:?}",
            first_row + 1,
            row_text(first_row + 1)
        );
        assert!(
            row_text(first_row + 2).contains("Line Three"),
            "row {} = {:?}",
            first_row + 2,
            row_text(first_row + 2)
        );
    }

    /// When the box is too short to fit every line, overflow lines are
    /// dropped and never painted past the box border.
    #[test]
    fn label_overflow_is_clamped_not_painted_past_the_box() {
        // A short box (action button reserves a row too) leaves only one row
        // for the label even though it carries two lines.
        let area = Rect::new(0, 0, 40, 5);
        let mut buf = Buffer::empty(area);
        let view = PipelineView {
            id: WidgetId::new("pipe"),
            stages: vec![PipelineStage {
                label: "Line One\nLine Two".into(),
                status: StageStatus::Active,
                action: Some("Go".into()),
            }],
            focused_stage: None,
        };
        let layout = draw_pipeline_view(&mut buf, area, &view, &Theme::default());

        let bb = layout.stages[0].box_bounds;
        let by = bb.y.round() as u16;
        let bh = bb.height.round() as u16;
        let (first_row, label_bottom) = label_row_bounds(by, bh, layout.stages[0].action_bounds);
        // Test setup assumption: the label area really is clamped to a
        // single row by icon+action reservation, so this test actually
        // exercises the clamp path.
        assert!(
            label_bottom.saturating_sub(first_row) <= 1,
            "test setup expects a 1-row label area, got {}..{}",
            first_row,
            label_bottom
        );

        let row_text =
            |row: u16| -> String { (0..area.width).map(|x| cell_char(&buf, x, row)).collect() };

        assert!(
            row_text(first_row).contains("Line One"),
            "row {first_row} = {:?}",
            row_text(first_row)
        );

        // The second line must never appear anywhere — including not
        // bleeding onto or past the bottom border row.
        for y in 0..bh {
            let row = by + y;
            assert!(
                !row_text(row).contains("Line Two"),
                "row {row} unexpectedly contains the overflow line: {:?}",
                row_text(row)
            );
        }
    }

    /// When no stage is focused the indicator row stays blank.
    #[test]
    fn no_indicator_when_not_focused() {
        let area = Rect::new(0, 0, 30, 6);
        let mut buf = Buffer::empty(area);
        let view = make_view(); // focused_stage = None
        let layout = draw_pipeline_view(&mut buf, area, &view, &Theme::default());

        let bb = layout.stages[0].box_bounds;
        let bx = bb.x.round() as u16;
        let bw = bb.width.round() as u16;
        let ind_col = bx + bw / 2;

        // Indicator row should be blank since nothing is focused.
        assert_eq!(cell_char(&buf, ind_col, 0), ' ');
    }

    /// The widget's contract cell paint reaches the buffer through the
    /// `CellSurface` verbs alone and paints what the TUI entry point does.
    #[test]
    fn contract_cell_paint_needs_only_the_verbs() {
        use crate::primitive::{LayoutCtx, Primitive};
        let w = make_view();
        let theme = Theme::default();
        let style = crate::Style::default();
        let area = Rect::new(0, 0, 72, 14);
        let mut legacy = Buffer::empty(area);
        draw_pipeline_view(&mut legacy, area, &w, &theme);
        let mut buf = Buffer::empty(area);
        let rect = crate::event::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        );
        let layout = Primitive::layout(&w, &LayoutCtx::new(1.0, 1.0, false), rect);
        w.paint_cells(
            &layout,
            &mut crate::tui::cell_surface::VerbsOnly(super::BufferCellSurface::new(&mut buf)),
            &crate::PaintEnv::new(&theme, &style),
        );
        assert_eq!(buf, legacy);
    }
}
