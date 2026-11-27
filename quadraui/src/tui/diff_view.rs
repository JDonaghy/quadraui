//! TUI entry point for [`crate::primitives::diff_view::DiffView`]: lays
//! the view out at one cell per row and paints it through the widget's
//! own cell painter over a [`ratatui::buffer::Buffer`].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::BufferCellSurface;
use crate::event::Rect as ERect;
#[cfg(test)]
use crate::primitives::diff_view::DiffMode;
use crate::primitives::diff_view::{DiffView, DiffViewLayout};
use crate::theme::Theme;

/// Paints `view` into `area` and returns its layout.
pub fn draw_diff_view(
    buf: &mut Buffer,
    area: Rect,
    view: &DiffView,
    theme: &Theme,
) -> DiffViewLayout {
    if area.width == 0 || area.height == 0 {
        return DiffViewLayout {
            visible_rows: 0,
            total_rows: view.total_rows(),
        };
    }
    let viewport = ERect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    );
    let geometry = view.layout(viewport, 1.0);
    crate::primitives::diff_view::paint_cells::paint(
        &mut BufferCellSurface::new(buf),
        view,
        &geometry,
        theme,
    );
    geometry.as_layout()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::diff_view::{DiffEditability, DiffHunk, DiffPane, DiffRow, DiffRowKind};
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn cell_row_str(buf: &Buffer, y: u16, width: u16) -> String {
        (0..width).map(|x| cell_char(buf, x, y)).collect()
    }

    /// Build a minimal `DiffView` with two hunks (2 rows each) in the given mode.
    fn make_view(mode: DiffMode) -> DiffView {
        DiffView {
            id: WidgetId::new("test"),
            left: String::new(),
            right: String::new(),
            left_label: None,
            right_label: None,
            hunks: vec![
                DiffHunk {
                    left_start: 1,
                    right_start: 1,
                    rows: vec![
                        DiffRow {
                            left: Some("alpha".into()),
                            right: Some("ALPHA".into()),
                            kind: DiffRowKind::Changed,
                        },
                        DiffRow {
                            left: Some("beta".into()),
                            right: Some("beta".into()),
                            kind: DiffRowKind::Same,
                        },
                    ],
                },
                DiffHunk {
                    left_start: 10,
                    right_start: 10,
                    rows: vec![
                        DiffRow {
                            left: Some("gamma".into()),
                            right: None,
                            kind: DiffRowKind::Removed,
                        },
                        DiffRow {
                            left: None,
                            right: Some("delta".into()),
                            kind: DiffRowKind::Added,
                        },
                    ],
                },
            ],
            mode,
            editability: DiffEditability::ReadOnly,
            scroll_offset: 0,
            focused_pane: DiffPane::Left,
            has_focus: false,
        }
    }

    // ── Zero-size guard ───────────────────────────────────────────────────────

    /// Zero-width area must not panic and must return empty layout.
    #[test]
    fn zero_size_area_side_by_side_returns_empty_layout_without_panic() {
        let buf_area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(buf_area);
        let view = make_view(DiffMode::SideBySide);
        let layout = draw_diff_view(&mut buf, Rect::new(0, 0, 0, 0), &view, &Theme::default());
        assert_eq!(layout.visible_rows, 0);
        assert_eq!(layout.total_rows, view.total_rows());
    }

    /// Zero-height area in unified mode must not panic and must return empty layout.
    #[test]
    fn zero_size_area_unified_returns_empty_layout_without_panic() {
        let buf_area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(buf_area);
        let view = make_view(DiffMode::Unified);
        let layout = draw_diff_view(&mut buf, Rect::new(0, 0, 0, 0), &view, &Theme::default());
        assert_eq!(layout.visible_rows, 0);
        assert_eq!(layout.total_rows, view.total_rows());
    }

    // ── Side-by-side scroll ───────────────────────────────────────────────────

    /// With `scroll_offset = 0`, the first row's left text starts on screen row 0.
    #[test]
    fn scroll_offset_zero_paints_first_row_at_top() {
        let area = Rect::new(0, 0, 20, 5);
        let mut buf = Buffer::empty(area);
        let view = make_view(DiffMode::SideBySide);
        draw_diff_view(&mut buf, area, &view, &Theme::default());

        // "alpha" should appear somewhere on row 0 (the left pane).
        let row0 = cell_row_str(&buf, 0, area.width);
        assert!(
            row0.contains("alpha"),
            "expected 'alpha' on row 0, got: {row0:?}"
        );
    }

    /// With `scroll_offset = 1`, the second row's left text starts on screen row 0.
    #[test]
    fn scroll_offset_one_paints_second_row_at_top() {
        let area = Rect::new(0, 0, 20, 5);
        let mut buf = Buffer::empty(area);
        let mut view = make_view(DiffMode::SideBySide);
        view.scroll_offset = 1;
        draw_diff_view(&mut buf, area, &view, &Theme::default());

        // Row 1 is "beta" — it should now be on screen row 0.
        let row0 = cell_row_str(&buf, 0, area.width);
        assert!(
            row0.contains("beta"),
            "expected 'beta' on row 0 at offset=1, got: {row0:?}"
        );
        // "alpha" (offset=0 row) must NOT appear.
        assert!(
            !row0.contains("alpha"),
            "unexpected 'alpha' on row 0 at offset=1: {row0:?}"
        );
    }

    // ── Unified mode ──────────────────────────────────────────────────────────

    /// In unified mode, the `@@` header for the first hunk appears on screen row 0.
    #[test]
    fn unified_hunk_header_appears_before_first_row() {
        let area = Rect::new(0, 0, 30, 6);
        let mut buf = Buffer::empty(area);
        let view = make_view(DiffMode::Unified);
        draw_diff_view(&mut buf, area, &view, &Theme::default());

        let row0 = cell_row_str(&buf, 0, area.width);
        assert!(
            row0.contains("@@"),
            "expected '@@ header' on row 0 in unified mode, got: {row0:?}"
        );
    }

    /// In unified mode, `layout.total_rows` counts hunk headers too, so
    /// scrolling to the last offset exposes the final content row.
    #[test]
    fn unified_scroll_reaches_last_row() {
        // 2 hunks × 2 rows = 4 content rows + 2 headers = 6 display lines.
        // With visible_rows = 2, max valid offset = 6 - 2 = 4.
        let area = Rect::new(0, 0, 30, 2);
        let mut buf = Buffer::empty(area);
        let view = make_view(DiffMode::Unified);

        // First pass: capture the correct total from the layout.
        let layout = draw_diff_view(&mut buf, area, &view, &Theme::default());
        assert_eq!(layout.total_rows, 6, "2 hunks × 2 rows + 2 headers = 6");
        assert_eq!(layout.visible_rows, 2);

        // Scroll to the maximum valid offset and verify last content is visible.
        let max_offset = layout.total_rows.saturating_sub(layout.visible_rows);
        let mut buf2 = Buffer::empty(area);
        let mut view2 = make_view(DiffMode::Unified);
        view2.scroll_offset = max_offset;
        draw_diff_view(&mut buf2, area, &view2, &Theme::default());

        // The last display line is the "delta" Added row (row index 5).
        // At offset = 4, screen row 1 should show "delta".
        let row1 = cell_row_str(&buf2, 1, area.width);
        assert!(
            row1.contains("delta"),
            "expected 'delta' on screen row 1 at max_offset={max_offset}, got: {row1:?}"
        );
    }

    /// Regression guard for the unified `@@` header line counts. A hunk
    /// with 2 removed + 1 added (one padding row) must emit
    /// `@@ -L,2 +R,1 @@`, not `@@ -L,3 +R,3 @@`. The previous
    /// implementation used `hunk.rows.len()` for both numbers, which
    /// inflated both counts whenever padding rows were present.
    #[test]
    fn unified_header_counts_exclude_padding() {
        let view = DiffView {
            id: WidgetId::new("hdr"),
            left: String::new(),
            right: String::new(),
            left_label: None,
            right_label: None,
            hunks: vec![crate::primitives::diff_view::DiffHunk {
                left_start: 5,
                right_start: 7,
                rows: vec![
                    // 2 removed + 1 added = 3 aligned rows (1 padding).
                    DiffRow {
                        left: Some("old1".into()),
                        right: Some("new1".into()),
                        kind: DiffRowKind::Changed,
                    },
                    DiffRow {
                        left: Some("old2".into()),
                        right: None,
                        kind: DiffRowKind::Removed,
                    },
                    DiffRow {
                        left: None,
                        right: Some("new3".into()),
                        kind: DiffRowKind::Added,
                    },
                ],
            }],
            mode: DiffMode::Unified,
            editability: DiffEditability::ReadOnly,
            scroll_offset: 0,
            focused_pane: DiffPane::Left,
            has_focus: false,
        };
        // Left rows = 2 (Changed has left + Removed has left).
        // Right rows = 2 (Changed has right + Added has right).
        let area = Rect::new(0, 0, 40, 4);
        let mut buf = Buffer::empty(area);
        draw_diff_view(&mut buf, area, &view, &Theme::default());

        let row0 = cell_row_str(&buf, 0, area.width);
        assert!(
            row0.contains("@@ -5,2 +7,2 @@"),
            "expected header '@@ -5,2 +7,2 @@', got: {row0:?}"
        );
        // The buggy header would have read "@@ -5,3 +7,3 @@".
        assert!(
            !row0.contains("@@ -5,3 +7,3 @@"),
            "header still reports raw row count: {row0:?}"
        );
    }

    /// The widget's contract cell paint reaches the buffer through the
    /// `CellSurface` verbs alone and paints what the TUI entry point does.
    #[test]
    fn contract_cell_paint_needs_only_the_verbs_side_by_side() {
        use crate::primitive::{LayoutCtx, Primitive};
        let w = make_view(DiffMode::SideBySide);
        let theme = Theme::default();
        let style = crate::Style::default();
        let area = Rect::new(0, 0, 72, 14);
        let mut legacy = Buffer::empty(area);
        draw_diff_view(&mut legacy, area, &w, &theme);
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

    /// The widget's contract cell paint reaches the buffer through the
    /// `CellSurface` verbs alone and paints what the TUI entry point does.
    #[test]
    fn contract_cell_paint_needs_only_the_verbs_unified() {
        use crate::primitive::{LayoutCtx, Primitive};
        let w = make_view(DiffMode::Unified);
        let theme = Theme::default();
        let style = crate::Style::default();
        let area = Rect::new(0, 0, 72, 14);
        let mut legacy = Buffer::empty(area);
        draw_diff_view(&mut legacy, area, &w, &theme);
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
