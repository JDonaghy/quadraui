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
//! The focused stage (keyboard navigation) receives a highlighted border.

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

/// Compute the TUI cell-unit layout for a [`PipelineView`] without painting.
pub fn tui_pipeline_view_layout(view: &PipelineView, area: Rect) -> PipelineViewLayout {
    let action_h = if view.stages.iter().any(|s| s.action.is_some()) {
        TUI_ACTION_HEIGHT
    } else {
        0.0
    };
    view.layout(
        area.x as f32,
        area.y as f32,
        PipelineViewMeasure::new(
            area.width as f32,
            area.height as f32,
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
    let border_normal = muted;
    let border_focus = accent;

    for sb in &layout.stages {
        let stage = &view.stages[sb.index];
        let is_focused = view.focused_stage == Some(sb.index);
        let border_col = if is_focused {
            border_focus
        } else {
            border_normal
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

        // ── Label (row 2 inside box) ─────────────────────────────────────
        let label_row = by + 2.min(bh.saturating_sub(2));
        if label_row < by + bh.saturating_sub(1) && !stage.label.is_empty() {
            let avail = bw.saturating_sub(2) as usize;
            let label: &str = &stage.label;
            let trimmed = if label.len() > avail {
                &label[..avail]
            } else {
                label
            };
            let pad = avail.saturating_sub(trimmed.len()) / 2;
            let start_col = bx + 1 + pad as u16;
            let max_col = bx + bw.saturating_sub(1);
            for (i, ch) in trimmed.chars().enumerate() {
                let col = start_col + i as u16;
                if col >= max_col {
                    break;
                }
                set_cell(buf, col, label_row, ch, fg, bg);
            }
        }

        // ── Action button at bottom ──────────────────────────────────────
        if let (Some(ab), Some(action_text)) = (sb.action_bounds, &stage.action) {
            let ay = ab.y.round() as u16;
            if ay > by && ay < by + bh {
                let btn_label = format!("[{}]", action_text);
                let avail = bw.saturating_sub(2) as usize;
                let btn: &str = &btn_label;
                let trimmed = if btn.len() > avail {
                    &btn[..avail]
                } else {
                    btn
                };
                let pad = avail.saturating_sub(trimmed.len()) / 2;
                let start_col = bx + 1 + pad as u16;
                let max_col = bx + bw.saturating_sub(1);
                for (i, ch) in trimmed.chars().enumerate() {
                    let col = start_col + i as u16;
                    if col >= max_col {
                        break;
                    }
                    set_cell(buf, col, ay, ch, accent, bg);
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
        // Top-left corner of first stage box.
        assert_eq!(cell_char(&buf, 0, 0), '┌');
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
}
