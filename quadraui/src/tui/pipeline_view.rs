//! TUI rasteriser for [`crate::primitives::pipeline_view::PipelineView`].
//!
//! Paints a horizontal row of bordered stage boxes connected by `───▶`
//! arrow connectors. Each box shows a status icon on the first row, the
//! stage label on the second row, and an optional `[Action]` button on
//! the bottom row.
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
use ratatui::style::Color as RatatuiColor;

use super::{ratatui_color, set_cell};
use crate::primitives::pipeline_view::{
    PipelineView, PipelineViewLayout, PipelineViewMeasure, StageStatus,
};
use crate::theme::Theme;

/// Width of the arrow connector in TUI cells.
const TUI_ARROW_WIDTH: f32 = 4.0;
/// Height reserved for the action button row (cells). `0` = no actions.
const TUI_ACTION_HEIGHT: f32 = 1.0;
/// Height reserved above stage boxes for the focus indicator (cells).
const TUI_FOCUS_INDICATOR_H: u16 = 1;

/// Compute the TUI cell-unit layout for a [`PipelineView`] without painting.
pub fn tui_pipeline_view_layout(view: &PipelineView, area: Rect) -> PipelineViewLayout {
    let action_h = if view.stages.iter().any(|s| s.action.is_some()) {
        TUI_ACTION_HEIGHT
    } else {
        0.0
    };
    // Note: the returned layout (incl. `bounds`) is offset down by
    // `TUI_FOCUS_INDICATOR_H`, so `bounds.y` starts below the reserved caret
    // row. The focus caret is drawn in that reserved row (above `bounds.y`);
    // a host that clips drawing to `layout.bounds` would clip it — clip to the
    // original `area` instead.
    view.layout(
        area.x as f32,
        (area.y + TUI_FOCUS_INDICATOR_H) as f32,
        PipelineViewMeasure::new(
            area.width as f32,
            area.height.saturating_sub(TUI_FOCUS_INDICATOR_H) as f32,
            TUI_ARROW_WIDTH,
            action_h,
        ),
    )
}

/// Draw a [`PipelineView`] into `area` on `buf`. Returns the layout for
/// host click dispatch.
pub fn draw_pipeline_view(
    buf: &mut Buffer,
    area: Rect,
    view: &PipelineView,
    theme: &Theme,
) -> PipelineViewLayout {
    let layout = tui_pipeline_view_layout(view, area);

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let bg = ratatui_color(theme.surface_bg);
    let fg = ratatui_color(theme.foreground);
    let muted = ratatui_color(theme.muted_fg);
    let accent = ratatui_color(theme.accent_bg);

    for sb in &layout.stages {
        let stage = &view.stages[sb.index];
        let is_focused = view.focused_stage == Some(sb.index);
        // Per-status border colour. Focus no longer overrides it — a ▼
        // indicator drawn in the reserved row above the box signals focus.
        let border_col = match stage.status {
            StageStatus::Active => ratatui_color(theme.accent_bg),
            StageStatus::Done => ratatui_color(theme.git_added),
            StageStatus::Failed => ratatui_color(theme.error_fg),
            // Stale gets a dim border so it visually retreats — the prior
            // verdict is shown but de-emphasised to signal "no longer trusted."
            StageStatus::Stale | StageStatus::Pending | StageStatus::Skipped => muted,
        };

        let bx = sb.box_bounds.x.round() as u16;
        let by = sb.box_bounds.y.round() as u16;
        let bw = sb.box_bounds.width.round() as u16;
        let bh = sb.box_bounds.height.round() as u16;

        if bw == 0 || bh == 0 {
            continue;
        }

        // ── Draw box border ───────────────────────────────────────────────
        // Top edge.
        set_cell(buf, bx, by, '┌', border_col, bg);
        for dx in 1..bw.saturating_sub(1) {
            set_cell(buf, bx + dx, by, '─', border_col, bg);
        }
        if bw >= 2 {
            set_cell(buf, bx + bw - 1, by, '┐', border_col, bg);
        }

        // Bottom edge.
        if bh >= 2 {
            let yb = by + bh - 1;
            set_cell(buf, bx, yb, '└', border_col, bg);
            for dx in 1..bw.saturating_sub(1) {
                set_cell(buf, bx + dx, yb, '─', border_col, bg);
            }
            if bw >= 2 {
                set_cell(buf, bx + bw - 1, yb, '┘', border_col, bg);
            }
        }

        // Side edges.
        for dy in 1..bh.saturating_sub(1) {
            set_cell(buf, bx, by + dy, '│', border_col, bg);
            if bw >= 2 {
                set_cell(buf, bx + bw - 1, by + dy, '│', border_col, bg);
            }
        }

        // Fill interior background.
        for dy in 1..bh.saturating_sub(1) {
            for dx in 1..bw.saturating_sub(1) {
                set_cell(buf, bx + dx, by + dy, ' ', fg, bg);
            }
        }

        // ── Status icon (row 1 inside box, or centred if single-row) ─────
        let (icon, icon_color) = status_icon(stage, theme);
        let icon_row = by + 1;
        if icon_row < by + bh.saturating_sub(1) {
            let icon_col = bx + bw / 2;
            if icon_col > bx && icon_col < bx + bw - 1 {
                set_cell(buf, icon_col, icon_row, icon, icon_color, bg);
            }
        }

        // ── Label (rows 2+ inside box) ───────────────────────────────────
        // The label may carry a newline (e.g. "Review T12\n3:45": stage+turns
        // on line 1, elapsed mm:ss on line 2). Render each line on its own row,
        // centred, with char-based (not byte-based) truncation so a stray byte
        // boundary never splits a multi-byte char and the second line never
        // overflows row 1 (the cause of the off-box "black gap" glitch).
        let first_row = by + 2.min(bh.saturating_sub(2));
        let inner_bottom = by + bh.saturating_sub(1); // border row (exclusive)
        if first_row < inner_bottom && !stage.label.is_empty() {
            let avail = bw.saturating_sub(2) as usize;
            let max_col = bx + bw.saturating_sub(1);
            for (line_idx, line) in stage.label.split('\n').take(2).enumerate() {
                let row = first_row + line_idx as u16;
                if row >= inner_bottom {
                    break;
                }
                let chars: Vec<char> = line.chars().collect();
                let take = chars.len().min(avail);
                let pad = avail.saturating_sub(take) / 2;
                let start_col = bx + 1 + pad as u16;
                for (i, ch) in chars.iter().take(take).enumerate() {
                    let col = start_col + i as u16;
                    if col >= max_col {
                        break;
                    }
                    set_cell(buf, col, row, *ch, fg, bg);
                }
            }
        }

        // ── Action button at bottom ──────────────────────────────────────
        if let (Some(ab), Some(action_text)) = (sb.action_bounds, &stage.action) {
            let ay = ab.y.round() as u16;
            if ay > by && ay < by + bh {
                let btn_label = format!("[{}]", action_text);
                let avail = bw.saturating_sub(2) as usize;
                let chars: Vec<char> = btn_label.chars().collect();
                let take = chars.len().min(avail);
                let pad = avail.saturating_sub(take) / 2;
                let start_col = bx + 1 + pad as u16;
                let max_col = bx + bw.saturating_sub(1);
                for (i, ch) in chars.iter().take(take).enumerate() {
                    let col = start_col + i as u16;
                    if col >= max_col {
                        break;
                    }
                    set_cell(buf, col, ay, *ch, accent, bg);
                }
            }
        }

        // ── Arrow connector to the right ─────────────────────────────────
        if let Some(arrow) = sb.arrow_bounds {
            let ax = arrow.x.round() as u16;
            let mid_y = by + bh / 2;
            // Draw: ─── ▶  (3 dashes then arrowhead)
            let aw = arrow.width.round() as u16;
            for dx in 0..aw.saturating_sub(1) {
                set_cell(buf, ax + dx, mid_y, '─', muted, bg);
            }
            if aw >= 1 {
                set_cell(buf, ax + aw - 1, mid_y, '▶', muted, bg);
            }
        }

        // ── Focus indicator (drawn in the reserved row above the box) ────
        if is_focused && by > 0 {
            set_cell(buf, bx + bw / 2, by - 1, '▼', muted, bg);
        }
    }

    layout
}

fn status_icon(
    stage: &crate::primitives::pipeline_view::PipelineStage,
    theme: &Theme,
) -> (char, RatatuiColor) {
    match stage.status {
        StageStatus::Done => ('✓', ratatui_color(theme.git_added)),
        StageStatus::Active => ('●', ratatui_color(theme.accent_bg)),
        StageStatus::Failed => ('✗', ratatui_color(theme.error_fg)),
        StageStatus::Pending => ('·', ratatui_color(theme.muted_fg)),
        StageStatus::Skipped => ('─', ratatui_color(theme.muted_fg)),
        // ↻ suggests re-running. Stale renders dim like Pending so its
        // prior verdict doesn't compete with a fresh Done downstream.
        StageStatus::Stale => ('↻', ratatui_color(theme.muted_fg)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::pipeline_view::{PipelineHit, PipelineStage};
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
        assert_eq!(cell_char(&buf, 0, 1), '┌');
    }

    #[test]
    fn layout_hit_test_action_round_trip() {
        let area = Rect::new(0, 0, 40, 5);
        let mut buf = Buffer::empty(area);
        let view = make_view();
        let layout = draw_pipeline_view(&mut buf, area, &view, &Theme::default());

        // Stage 1 has action bounds.
        let ab = layout.stages[1]
            .action_bounds
            .expect("action bounds for stage 1");
        let hit = layout.hit_test(ab.x + ab.width / 2.0, ab.y + ab.height / 2.0);
        assert_eq!(hit, PipelineHit::Action(1));
    }

    #[test]
    fn layout_hit_test_body_round_trip() {
        let area = Rect::new(0, 0, 40, 5);
        let mut buf = Buffer::empty(area);
        let view = make_view();
        let layout = draw_pipeline_view(&mut buf, area, &view, &Theme::default());

        let bb = layout.stages[0].box_bounds;
        let hit = layout.hit_test(bb.x + 1.0, bb.y + 1.0);
        assert_eq!(hit, PipelineHit::Body(0));
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

        // The box border is still drawn (status colour, not overridden).
        assert_eq!(cell_char(&buf, bx, by), '┌');
    }

    /// A label carrying a newline renders as two centred rows inside the box —
    /// no literal newline cell on row 1, second line on its own row. Regression
    /// guard for the off-box "black gap" glitch (coord-tui #280).
    #[test]
    fn two_line_label_renders_on_separate_rows() {
        let area = Rect::new(0, 0, 40, 8);
        let mut buf = Buffer::empty(area);
        let view = PipelineView {
            id: WidgetId::new("pipe"),
            stages: vec![PipelineStage {
                label: "Review T12\n3:45".into(),
                status: StageStatus::Active,
                action: None,
            }],
            focused_stage: None,
        };
        let layout = draw_pipeline_view(&mut buf, area, &view, &Theme::default());

        let bb = layout.stages[0].box_bounds;
        let by = bb.y.round() as u16;
        let bh = bb.height.round() as u16;
        let first_row = by + 2.min(bh.saturating_sub(2));

        // Collect the rendered glyphs of the two label rows.
        let row1: String = (0..area.width).map(|x| cell_char(&buf, x, first_row)).collect();
        let row2: String = (0..area.width).map(|x| cell_char(&buf, x, first_row + 1)).collect();

        // Line 1 holds the stage + turn count; line 2 holds the elapsed time.
        assert!(row1.contains("Review T12"), "row1 = {row1:?}");
        assert!(row2.contains("3:45"), "row2 = {row2:?}");
        // The elapsed segment must NOT bleed onto row 1 (the old glitch).
        assert!(!row1.contains("3:45"), "elapsed leaked onto row1: {row1:?}");
        // No literal newline rendered as a cell anywhere on row 1.
        assert!(!row1.contains('\n'), "newline rendered as a cell: {row1:?}");
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
}
