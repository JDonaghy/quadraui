//! Cell painter for [`PipelineView`], written against [`CellSurface`].
//!
//! Paints a horizontal row of rounded stage boxes connected by `───▶`
//! arrow connectors. Each box shows a status icon on its first row, the
//! stage label on as many rows as fit below it, and an optional `[Action]`
//! button on its bottom row. The focused stage shows a `▼` caret in the
//! row reserved above the box.

use crate::cell_surface::{solid_style, CellSurface};
use crate::primitives::pipeline_view::{
    status_color, status_glyph, PipelineView, PipelineViewLayout,
};
use crate::theme::Theme;
use crate::types::Color;

/// Compute the first (inclusive) and last (exclusive) TUI row available for
/// the stage label, given the box's top row (`by`), its height in rows
/// (`bh`), and the stage's action-button bounds (if any).
///
/// `label_top` is pinned directly below the icon's single rendered row
/// (`icon_row = by + 1`, see [`paint`]) to avoid a gap
/// between the icon and label as box height grows.
///
/// `label_bottom` stops before whichever comes first: the box's bottom
/// border row, or the action-button row (if the stage has one) — i.e. the
/// rows that are actually painted, not a recomputed proportional height.
pub(crate) fn label_row_bounds(
    by: u16,
    bh: u16,
    action_bounds: Option<crate::event::Rect>,
) -> (u16, u16) {
    let label_top = by + 2.min(bh.saturating_sub(2));
    let inner_bottom = by + bh.saturating_sub(1); // border row (exclusive)
    let label_bottom = match action_bounds {
        Some(ab) => (ab.y.round() as u16).min(inner_bottom),
        None => inner_bottom,
    };
    (label_top, label_bottom)
}

/// Paints `view` over `layout`, which [`PipelineView::cell_layout`]
/// computed.
pub(crate) fn paint(
    cells: &mut dyn CellSurface,
    view: &PipelineView,
    layout: &PipelineViewLayout,
    theme: &Theme,
) {
    if layout.bounds.width < 1.0 || layout.bounds.height < 1.0 {
        return;
    }

    let bg = theme.surface_bg;
    let fg = theme.foreground;
    let muted = theme.muted_fg;
    let accent = theme.accent_bg;
    let mut set_cell = |x: u16, y: u16, ch: char, fg: Color, bg: Color| {
        cells.put_char(x, y, ch, solid_style(fg, bg));
    };

    for sb in &layout.stages {
        let stage = &view.stages[sb.index];
        let is_focused = view.focused_stage == Some(sb.index);
        // Per-status border colour. Focus does not override it — a ▼
        // indicator drawn in the reserved row above the box signals focus.
        // Stale gets the same dim (muted) border as Pending/Skipped so it
        // visually retreats — the prior verdict is shown but de-emphasised.
        let border_col = status_color(&stage.status, theme);

        let bx = sb.box_bounds.x.round() as u16;
        let by = sb.box_bounds.y.round() as u16;
        let bw = sb.box_bounds.width.round() as u16;
        let bh = sb.box_bounds.height.round() as u16;

        if bw == 0 || bh == 0 {
            continue;
        }

        // ── Draw box border (rounded corners) ────────────────────────────
        // Top edge — use rounded-corner glyphs (╭ ╮ ╰ ╯) so the box reads
        // softer, matching the CSS `border-radius` used by the GTK rasteriser.
        set_cell(bx, by, '╭', border_col, bg);
        for dx in 1..bw.saturating_sub(1) {
            set_cell(bx + dx, by, '─', border_col, bg);
        }
        if bw >= 2 {
            set_cell(bx + bw - 1, by, '╮', border_col, bg);
        }

        // Bottom edge.
        if bh >= 2 {
            let yb = by + bh - 1;
            set_cell(bx, yb, '╰', border_col, bg);
            for dx in 1..bw.saturating_sub(1) {
                set_cell(bx + dx, yb, '─', border_col, bg);
            }
            if bw >= 2 {
                set_cell(bx + bw - 1, yb, '╯', border_col, bg);
            }
        }

        // Side edges.
        for dy in 1..bh.saturating_sub(1) {
            set_cell(bx, by + dy, '│', border_col, bg);
            if bw >= 2 {
                set_cell(bx + bw - 1, by + dy, '│', border_col, bg);
            }
        }

        // Fill interior background.
        for dy in 1..bh.saturating_sub(1) {
            for dx in 1..bw.saturating_sub(1) {
                set_cell(bx + dx, by + dy, ' ', fg, bg);
            }
        }

        // ── Status icon (row 1 inside box, or centred if single-row) ─────
        // Skipped's glyph is a single-line dash (─) drawn dim like
        // Pending; Stale's `↻` also renders dim so its prior verdict
        // doesn't compete visually with a fresh Done downstream.
        let icon = status_glyph(&stage.status).chars().next().unwrap_or(' ');
        let icon_color = status_color(&stage.status, theme);
        let icon_row = by + 1;
        if icon_row < by + bh.saturating_sub(1) {
            let icon_col = bx + bw / 2;
            if icon_col > bx && icon_col < bx + bw - 1 {
                set_cell(icon_col, icon_row, icon, icon_color, bg);
            }
        }

        // ── Label (arbitrary N lines, directly below the icon row) ───────
        // The label may carry any number of newlines (e.g. "Review T12\n3:45\n+2"
        // — stage+turns, elapsed mm:ss, extra detail each on their own line).
        // Render every line, centred, with char-based (not byte-based) truncation
        // so a stray byte boundary never splits a multi-byte char.
        //
        // label_top is pinned right after the icon row (contiguous), and
        // label_bottom is derived from the box border / action button rows
        // actually painted — not from proportional layout bounds.
        let (label_top, label_bottom) = label_row_bounds(by, bh, sb.action_bounds);
        if label_top < label_bottom && !stage.label.is_empty() {
            let avail = bw.saturating_sub(2) as usize;
            let max_col = bx + bw.saturating_sub(1);
            for (line_idx, line) in stage.label.split('\n').enumerate() {
                let row = label_top + line_idx as u16;
                if row >= label_bottom {
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
                    set_cell(col, row, *ch, fg, bg);
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
                    set_cell(col, ay, *ch, accent, bg);
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
                set_cell(ax + dx, mid_y, '─', muted, bg);
            }
            if aw >= 1 {
                set_cell(ax + aw - 1, mid_y, '▶', muted, bg);
            }
        }

        // ── Focus indicator (drawn in the reserved row above the box) ────
        if is_focused && by > 0 {
            set_cell(bx + bw / 2, by - 1, '▼', muted, bg);
        }
    }
}
