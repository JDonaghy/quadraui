//! Direct2D / DirectWrite rasteriser for [`crate::DataTable`] (issue #26).
//!
//! Painting moved to the shared
//! [`crate::primitives::data_table::native_surface_paint::paint`] (#1084,
//! `PaintSurface` Phase 4 7/8) — see that fn's doc for the full
//! per-backend divergence survey this closed, most notably: this module
//! used to hand-roll flat, non-hover-aware scrollbar fills (see the old
//! "Scope for #26" note below) instead of the shared, translucent
//! [`crate::primitives::scrollbar::native_surface_paint::paint`]
//! `gtk::data_table` already used, approximated selection/hover row tints
//! with a CPU-side [`crate::types::Color::blend`] instead of a real alpha
//! composite, and silently discarded every footer span's own `fg`,
//! painting the whole footer cell in one colour. [`draw_data_table`]
//! below is now a thin wrapper over the shared paint, using
//! [`crate::win::surface::D2dSurface`] as the `PaintSurface` adapter —
//! mirroring [`crate::macos::data_table::draw_data_table`]'s equivalent
//! migration. `gtk::data_table::draw_data_table` is *not* migrated; see
//! the shared `paint`'s doc for why. [`win_data_table_layout`] (pure
//! geometry, no paint) is untouched.
//!
//! Issue #1078: only [`draw_data_table`] (the real Direct2D paint entry
//! point) is `#[cfg(target_os = "windows")]`-gated. [`win_data_table_layout`]
//! is pure geometry generic over
//! [`crate::primitives::layout_metrics::TextMeasure`] — no Direct2D/
//! DirectWrite type in its signature — so it compiles and runs
//! everywhere, including a plain `cargo test --features win` on Linux.
//! `super::mod`'s `mod data_table;` is no longer whole-module gated; see
//! `backend.rs`'s module docs. See `win::status_bar`'s module doc for why
//! colours come from `Theme::default()` rather than a live `WinBackend`
//! theme field.

#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use crate::event::Rect;
use crate::primitives::data_table::DataTable;
use crate::primitives::layout_metrics::{pixel_data_table_layout, TextMeasure};
#[cfg(target_os = "windows")]
use crate::theme::Theme;
use crate::DataTableLayout;

/// Compute a [`DataTable`]'s layout without painting — the DirectWrite
/// twin of [`draw_data_table`]'s internal layout call. Pure geometry over
/// [`TextMeasure`] (issue #1078) — `measure` may be a live `&DWrite` (when
/// painting) or [`super::backend`]'s nominal measurer (no surface yet).
/// Shares its column-measurement path with `gtk_data_table_layout` /
/// `mac_data_table_layout` via [`pixel_data_table_layout`] (issue #1079).
pub fn win_data_table_layout(
    measure: &dyn TextMeasure,
    rect: Rect,
    table: &DataTable,
    line_height: f32,
) -> DataTableLayout {
    pixel_data_table_layout(table, rect.width, rect.height, line_height, measure)
}

/// Draw a [`DataTable`] into `rect` (DIPs) on `target`. Returns the
/// resolved [`DataTableLayout`] for host click dispatch.
///
/// `hovered_idx` tints the hovered body row (skipped when it's also
/// the selected row). See
/// [`crate::primitives::data_table::native_surface_paint::paint`]'s doc
/// for the full visual contract this now shares with
/// `macos::data_table::draw_data_table`.
#[cfg(target_os = "windows")]
pub fn draw_data_table(
    target: &ID2D1RenderTarget,
    dwrite: &super::text::DWrite,
    rect: Rect,
    table: &DataTable,
    line_height: f32,
    hovered_idx: Option<usize>,
) -> DataTableLayout {
    let theme = Theme::default();
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    crate::primitives::data_table::native_surface_paint::paint(
        table,
        &mut surface,
        &theme,
        rect,
        line_height,
        hovered_idx,
    )
}

// #1078: every test below paints through a real `DWrite`/`HeadlessSurface`
// — gated the same way the whole module used to be, rather than
// pretending they run on Linux.
#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use crate::primitives::data_table::{Column, ColumnAlign, ColumnWidth, DataRow, DataTableHit};
    use crate::types::{Decoration, StyledText, WidgetId};
    use crate::win::testing::HeadlessSurface;
    use crate::win::text::DWrite;

    const W: f32 = 300.0;
    const H: f32 = 100.0;
    const LINE_HEIGHT: f32 = 16.0;

    fn table(rows: Vec<DataRow>) -> DataTable {
        DataTable {
            id: WidgetId::new("table"),
            columns: vec![
                Column {
                    title: "Name".into(),
                    width: ColumnWidth::Flex(1.0),
                    align: ColumnAlign::Left,
                },
                Column {
                    title: "Size".into(),
                    width: ColumnWidth::Flex(1.0),
                    align: ColumnAlign::Right,
                },
            ],
            rows,
            selected_idx: Some(0),
            scroll_offset: 0,
            sort: None,
            has_focus: true,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: Vec::new(),
            footer: None,
        }
    }

    fn row(name: &str, size: &str) -> DataRow {
        DataRow {
            cells: vec![
                StyledText::plain(name.to_string()),
                StyledText::plain(size.to_string()),
            ],
            decoration: Decoration::Normal,
        }
    }

    /// Paint↔click round trip: the selected row's blended background
    /// must be painted at its own bounds, and clicking each row/header
    /// resolves to the expected `DataTableHit`.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let table = table(vec![
            row("a.txt", "1kb"),
            row("b.txt", "2kb"),
            row("c.txt", "3kb"),
        ]);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_data_table(target, &dwrite, rect, &table, LINE_HEIGHT, None);
            })
            .map(|_| win_data_table_layout(&dwrite, rect, &table, LINE_HEIGHT))
            .expect("paint data table");

        // Header click.
        let hit = layout.hit_test(1.0, 1.0, table.scroll_offset, table.rows.len());
        assert_eq!(hit, DataTableHit::Header { col: 0 });

        // Row click (row 1, below the header).
        let row_y = layout.header_height + line_height_mid(LINE_HEIGHT);
        let hit = layout.hit_test(1.0, row_y, table.scroll_offset, table.rows.len());
        assert_eq!(hit, DataTableHit::Row { idx: 0 });

        // Selected row (idx 0) painted a blended background distinct
        // from the plain body background.
        let theme = Theme::default();
        let sample_y = (layout.header_height + 2.0) as u32;
        let px = surface.pixel_at(2, sample_y);
        assert_ne!(
            (px.r, px.g, px.b),
            (theme.background.r, theme.background.g, theme.background.b),
            "selected row should paint a tinted background distinct from the plain body bg"
        );
    }

    fn line_height_mid(h: f32) -> f32 {
        h / 2.0
    }

    /// Scroll-offset round trip: with `scroll_offset = 1`, a click on
    /// the first body row resolves to row index 1.
    #[test]
    fn scroll_offset_hit_test_agrees() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut t = table(vec![row("a", "1"), row("b", "2"), row("c", "3")]);
        t.scroll_offset = 1;
        let rect = Rect::new(0.0, 0.0, W, H);
        let layout = win_data_table_layout(&dwrite, rect, &t, LINE_HEIGHT);
        let row_y = layout.header_height + line_height_mid(LINE_HEIGHT);
        let hit = layout.hit_test(1.0, row_y, t.scroll_offset, t.rows.len());
        assert_eq!(hit, DataTableHit::Row { idx: 1 });
    }

    /// No-paint layout must agree byte-for-byte with what
    /// `draw_data_table` painted.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let table = table(vec![row("a.txt", "1kb"), row("b.txt", "2kb")]);
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_data_table(target, &dwrite, rect, &table, LINE_HEIGHT, None);
            })
            .map(|_| win_data_table_layout(&dwrite, rect, &table, LINE_HEIGHT))
            .expect("paint");
        let no_paint = win_data_table_layout(&dwrite, rect, &table, LINE_HEIGHT);
        assert_eq!(painted, no_paint);
    }

    /// #1084's RED-before-the-port case: pre-migration,
    /// `win::data_table::draw_data_table`'s footer painted the *entire*
    /// cell as one run in `theme.foreground`, discarding every span's own
    /// `fg` — there was no way for a footer span's colour override to
    /// ever reach the screen on this backend. The shared `paint` now
    /// splits by span, so the "Total" span's red `fg` must actually
    /// appear somewhere in the footer band.
    #[test]
    fn footer_span_colour_overrides_the_default_foreground() {
        use crate::types::{Color, StyledSpan};

        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut t = table(vec![row("a.txt", "1kb")]);
        t.footer = Some(DataRow {
            cells: vec![
                StyledText {
                    spans: vec![StyledSpan {
                        text: "Total".into(),
                        fg: Some(Color::rgb(255, 0, 0)),
                        bg: None,
                        bold: false,
                        italic: false,
                        underline: false,
                    }],
                },
                StyledText::plain("3"),
            ],
            decoration: Decoration::Normal,
        });
        let rect = Rect::new(0.0, 0.0, W, H);
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        surface
            .fill_rect(rect, Color::rgb(255, 255, 255))
            .expect("fill white bg");
        surface
            .paint(|target| {
                draw_data_table(target, &dwrite, rect, &t, LINE_HEIGHT, None);
            })
            .expect("paint data table");

        let layout = win_data_table_layout(&dwrite, rect, &t, LINE_HEIGHT);
        let footer_top = (H - layout.footer_height).max(0.0) as u32;

        let mut found_red = false;
        for y in footer_top..(H as u32) {
            for x in 0..(W as u32) {
                let px = surface.pixel_at(x, y);
                if px.r > 200 && px.g < 80 && px.b < 80 {
                    found_red = true;
                }
            }
        }
        assert!(
            found_red,
            "footer 'Total' span should paint in its own red fg, not \
             the pre-#1084 whole-cell `theme.foreground`"
        );
    }
}
