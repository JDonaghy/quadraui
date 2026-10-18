//! GTK rasteriser for [`crate::TreeView`].
//!
//! Content painting (background, rows, chevron/icon/badge/text or
//! inline-edit, vertical scrollbar) is shared via
//! [`crate::primitives::tree::native_surface_paint::paint`].
//!
//! Per-row heights are **non-uniform**: header rows use `line_height`,
//! leaves and ordinary branches use `(line_height * 1.4).round()` (the
//! established GTK convention) — unless the host set
//! [`TreeStyle::row_height`](crate::types::TreeStyle::row_height), which
//! pins the leaf/branch pitch independent of `line_height`. The
//! primitive's `tree.layout()` measurer reports each row's height so
//! the visible-row positions stack accurately.

use gtk4::cairo::Context;
use gtk4::pango;

use super::toolbar::PangoMeasure;
use crate::event::Rect as QRect;
use crate::primitives::tree::{TreeView, TreeViewLayout};
use crate::theme::Theme;

/// Compute the layout the GTK rasteriser would produce for `tree` in
/// `area` at `line_height`. Hosts and tests call this to drive
/// hit-testing without re-deriving row pitch (`1.0 × line_height` for
/// `Decoration::Header`, `1.4 × line_height` for everything else, unless
/// `tree.style.row_height` overrides the non-header pitch).
///
/// Mirrors TUI's [`crate::tui::tui_tree_layout`] in spirit; differs
/// in row pitch (TUI = 1 cell uniform; GTK = mixed via decoration).
///
/// Thin wrapper over [`crate::primitives::layout_metrics::tree_layout`]
/// — the row-pitch/chevron math is identical across every pixel
/// backend, so it lives there once instead of once per backend. The
/// chevron glyph width is an *estimate* (`line_height * 0.65`); callers
/// with a live `pango::Layout` should prefer [`gtk_tree_layout_measured`]
/// (`draw_tree` and `GtkBackend::tree_layout` both do).
pub fn gtk_tree_layout(tree: &TreeView, area: QRect, line_height: f64) -> TreeViewLayout {
    crate::primitives::layout_metrics::tree_layout(tree, area, line_height)
}

/// Like [`gtk_tree_layout`], but measures the chevron glyph actually
/// configured on `tree.style` (`chevron_expanded`/`chevron_collapsed`)
/// through Pango instead of estimating its pixel width as
/// `line_height * 0.65` — the same correctness `macos::tree::mac_tree_layout`
/// already gets from a real `CTFont`. `pango_layout` is `Some` inside a
/// draw frame (Pango can measure); pass `None` only when no live layout
/// is available, which falls back to `char_width`-based estimation.
pub fn gtk_tree_layout_measured(
    tree: &TreeView,
    area: QRect,
    line_height: f64,
    pango_layout: Option<&pango::Layout>,
    char_width: f64,
) -> TreeViewLayout {
    let measure = PangoMeasure {
        pango_layout,
        char_width,
    };
    crate::primitives::layout_metrics::tree_layout_measured(tree, area, line_height, &measure)
}

/// Draw a [`TreeView`] into `(x, y, w, h)` on `cr`. `nerd_fonts_enabled`
/// controls which icon variant the consumer's icon registry exposes.
///
/// Row labels/badges/chevrons are chrome, not editor content —
/// the caller is responsible for setting `layout`'s font
/// description to the desired UI font before calling and restoring
/// whatever it was afterward. This rasteriser has no separate
/// "editor font" concept of its own; it measures and paints with
/// whatever font is current on `layout`.
///
/// # Visual contract
///
/// - **Background:** [`Theme::tab_bar_bg`].
/// - **Header rows** (`Decoration::Header`):
///   [`Theme::header_bg`] / [`Theme::header_fg`], shorter row
///   (`line_height`).
/// - **Selected row** (when `tree.has_focus`): [`Theme::selected_bg`]
///   with [`Theme::header_fg`] text.
/// - **Muted row** (`Decoration::Muted`): [`Theme::muted_fg`] text on
///   the default row bg.
/// - **Other rows**: [`Theme::foreground`] text on
///   [`Theme::tab_bar_bg`]. Branches and leaves get the same row
///   styling — `is_expanded`-ness only affects chevron rendering.
/// - **Indent:** `(line_height * 0.9).round()` pixels per depth level.
/// - **Chevrons:** [`tree.style.chevron_expanded`] /
///   [`tree.style.chevron_collapsed`] for branches when
///   `tree.style.show_chevrons` is true, *unless* the field is still
///   holding `TreeStyle::default()`'s `"▾"`/`"▸"`, in which case the
///   shared paint path (`primitives::tree::paint`) substitutes the
///   sharper `codicon::CHEVRON_DOWN`/`CHEVRON_RIGHT` glyph instead; a
///   non-default override is always painted verbatim. Leaves get a
///   `line_height * 0.8` leading offset for visual alignment.
/// - **Badge** (right-aligned): rendered in `badge.fg`/`badge.bg`
///   (falling back to [`Theme::muted_fg`] / row bg) when there's
///   room past the text.
/// - **Icon:** painted in `icon.color` when set, else the row's
///   default fg.
#[allow(clippy::too_many_arguments)]
pub fn draw_tree(
    cr: &Context,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    tree: &TreeView,
    theme: &Theme,
    line_height: f64,
    nerd_fonts_enabled: bool,
) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    layout.set_attributes(None);

    let tree_layout = gtk_tree_layout_measured(
        tree,
        QRect::new(0.0, 0.0, w as f32, h as f32),
        line_height,
        Some(layout),
        0.0,
    );

    let area = crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32);
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(layout),
        translucent_fill: true,
    };
    crate::primitives::tree::native_surface_paint::paint(
        tree,
        area,
        &tree_layout,
        line_height as f32,
        nerd_fonts_enabled,
        &mut surface,
        theme,
    );

    layout.set_attributes(None);
}

// ── Tests ──────────────────────────────────────────────────────────────────
//
// Paint↔click round-trip harness for the GTK `draw_tree`. Mirrors the
// TUI tree harness pattern but paints into a `cairo::ImageSurface`
// instead of a ratatui `Buffer` and inspects pixels rather than glyphs.
//
// The bug class this catches: `scroll_offset` drift (paint advances by
// scroll_offset but click forgets, or vice versa) and row-pitch drift
// (mixed-decoration trees where headers are 1.0×line_height and leaves
// are 1.4×line_height stack inconsistently). GTK's `draw_tree` and
// `GtkBackend::tree_layout` both build on `gtk_tree_layout_measured`
// with a live Pango layout, so the contract is "they consume the same
// layout" — these tests verify that contract.
//
// Tests gated `#[cfg(all(test, feature = "gtk"))]` so they only run
// under `cargo test --features gtk`. They don't need a real display.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::tree::{TreeRow, TreeView, TreeViewHit};
    use crate::types::{Color, Decoration, SelectionMode, StyledText, TreeStyle, WidgetId};
    use pangocairo::cairo::{Context, Format, ImageSurface};

    const W: i32 = 200;
    const H: i32 = 200;
    const LINE_HEIGHT: f64 = 14.0;

    fn test_theme() -> Theme {
        Theme {
            tab_bar_bg: Color::rgb(255, 255, 255),
            background: Color::rgb(255, 255, 255),
            ..Theme::default()
        }
    }

    fn leaf(idx: u16, label: &str) -> TreeRow {
        TreeRow {
            path: vec![idx],
            indent: 0,
            icon: None,
            text: StyledText::plain(label.to_string()),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: None,
        }
    }

    fn header_row(idx: u16, label: &str) -> TreeRow {
        TreeRow {
            path: vec![idx],
            indent: 0,
            icon: None,
            text: StyledText::plain(label.to_string()),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Header,
            edit: None,
        }
    }

    fn make_tree(rows: Vec<TreeRow>) -> TreeView {
        TreeView {
            id: WidgetId::new("tree"),
            rows,
            selection_mode: SelectionMode::Single,
            selected_path: None,
            scroll_offset: 0.0,
            style: TreeStyle::default(),
            has_focus: true,
        }
    }

    /// Paint `tree` into a fresh surface; return (surface, layout).
    /// Hit-test queries the SAME layout the rasteriser used —
    /// that's the source-of-truth contract `gtk_tree_layout` enforces.
    fn paint_then_layout(tree: &TreeView) -> (ImageSurface, TreeViewLayout) {
        let surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().ok();
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_tree(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                W as f64,
                H as f64,
                tree,
                &test_theme(),
                LINE_HEIGHT,
                /* nerd_fonts */ false,
            );
        }
        let area = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = gtk_tree_layout(tree, area, LINE_HEIGHT);
        (surface, layout)
    }

    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

    fn is_painted(data: &[u8], stride: usize, x: i32, y: i32) -> bool {
        let (r, g, b) = pixel(data, stride, x, y);
        !(r == 255 && g == 255 && b == 255)
    }

    fn first_painted_in(
        data: &[u8],
        stride: usize,
        x_range: (i32, i32),
        y_range: (i32, i32),
    ) -> Option<(i32, i32)> {
        for y in y_range.0..y_range.1 {
            for x in x_range.0..x_range.1 {
                if x < 0 || y < 0 || x >= W || y >= H {
                    continue;
                }
                if is_painted(data, stride, x, y) {
                    return Some((x, y));
                }
            }
        }
        None
    }

    /// Round-trip: paint each row, find an interior painted pixel
    /// inside that row's bounds, hit_test there, assert `Row(idx)`.
    /// Catches paint-vs-layout drift in row pitch.
    #[test]
    fn gtk_clicks_land_on_painted_row() {
        let tree = make_tree(vec![
            leaf(0, "alpha"),
            leaf(1, "beta"),
            leaf(2, "gamma"),
            leaf(3, "delta"),
        ]);
        let (mut surface, layout) = paint_then_layout(&tree);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        for vis in &layout.visible_rows {
            let bounds = vis.bounds;
            // Interior-pixel scan: skip the 1px AA boundary because
            // GTK row bounds are fractional (line_height * 1.4 may
            // round to non-integer pixels per row).
            let x_range = (
                (bounds.x + 1.0).floor() as i32,
                (bounds.x + bounds.width - 1.0).floor() as i32,
            );
            let y_range = (
                (bounds.y + 1.0).floor() as i32,
                (bounds.y + bounds.height - 1.0).floor() as i32,
            );
            let painted = first_painted_in(&data, stride, x_range, y_range).unwrap_or_else(|| {
                panic!(
                    "row {} interior bounds (x {}..{}, y {}..{}) contained no painted pixel",
                    vis.row_idx, x_range.0, x_range.1, y_range.0, y_range.1
                )
            });
            let hit = layout.hit_test(painted.0 as f32 + 0.5, painted.1 as f32 + 0.5);
            match hit {
                TreeViewHit::Row(idx) => assert_eq!(
                    idx, vis.row_idx,
                    "pixel ({}, {}) painted in row {} but hit_test returned Row({})",
                    painted.0, painted.1, vis.row_idx, idx
                ),
                other => panic!(
                    "pixel ({}, {}) painted in row {} but hit_test returned {:?}",
                    painted.0, painted.1, vis.row_idx, other
                ),
            }
        }
    }

    /// Click below the last painted row returns `Empty`. Locks the
    /// "tree's empty space below content is not row-hittable" contract.
    #[test]
    fn gtk_click_below_last_row_returns_empty() {
        let tree = make_tree(vec![leaf(0, "alpha"), leaf(1, "beta"), leaf(2, "gamma")]);
        let (_surface, layout) = paint_then_layout(&tree);
        let last = layout.visible_rows.last().expect("tree has visible rows");
        let click_y = last.bounds.y + last.bounds.height + 5.0;
        assert!(
            click_y < H as f32,
            "test setup: click_y must be inside surface bounds"
        );
        let hit = layout.hit_test(W as f32 / 2.0, click_y);
        assert!(
            matches!(hit, TreeViewHit::Empty),
            "click below last row should return Empty, got {:?}",
            hit
        );
    }

    /// Round-trip across `scroll_offset`: when the host scrolls 3
    /// rows down, `visible_rows[0]` is `tree.rows[3]`, the painted
    /// first row corresponds to "v3", and hit_test at that row's
    /// painted center returns `Row(3)`. Catches the bug class where
    /// paint advances by scroll_offset but click forgets (or vice
    /// versa).
    #[test]
    fn gtk_scroll_offset_paint_and_click_agree() {
        let labels = ["r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9"];
        let mut tree = make_tree(
            labels
                .iter()
                .enumerate()
                .map(|(i, l)| leaf(i as u16, l))
                .collect(),
        );
        tree.scroll_offset = 3.0;
        let (mut surface, layout) = paint_then_layout(&tree);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        // After scroll, first visible row should be index 3.
        let first = layout
            .visible_rows
            .first()
            .expect("scrolled tree still has visible rows");
        assert_eq!(
            first.row_idx, 3,
            "scroll_offset=3 should put rows[3] at the top; got rows[{}]",
            first.row_idx
        );

        // Find an interior painted pixel in that row and hit_test.
        let bounds = first.bounds;
        let x_range = (
            (bounds.x + 1.0).floor() as i32,
            (bounds.x + bounds.width - 1.0).floor() as i32,
        );
        let y_range = (
            (bounds.y + 1.0).floor() as i32,
            (bounds.y + bounds.height - 1.0).floor() as i32,
        );
        let painted = first_painted_in(&data, stride, x_range, y_range).unwrap_or_else(|| {
            panic!(
                "scrolled-to-top row 3 contained no painted pixel in interior bounds ({:?})",
                bounds
            )
        });
        let hit = layout.hit_test(painted.0 as f32 + 0.5, painted.1 as f32 + 0.5);
        assert!(
            matches!(hit, TreeViewHit::Row(3)),
            "post-scroll click on painted top row returned {:?}; expected Row(3)",
            hit
        );
    }

    /// Mixed-decoration round-trip: a tree alternating
    /// `Decoration::Header` (1.0 × line_height) and `Decoration::Normal`
    /// (1.4 × line_height) rows. Every visible row's painted center
    /// pixel must hit_test to that row's index. Catches row-pitch
    /// drift where one path uses uniform pitch while the other
    /// honours the 1×/1.4× boundary.
    #[test]
    fn gtk_mixed_decoration_rows_round_trip() {
        let tree = make_tree(vec![
            header_row(0, "HEADER A"),
            leaf(1, "leaf-a-0"),
            leaf(2, "leaf-a-1"),
            header_row(3, "HEADER B"),
            leaf(4, "leaf-b-0"),
            leaf(5, "leaf-b-1"),
        ]);
        let (mut surface, layout) = paint_then_layout(&tree);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        for vis in &layout.visible_rows {
            let bounds = vis.bounds;
            // Click at the row's vertical center to avoid any AA at
            // the integer-pixel boundaries between adjacent rows.
            let click_x = (bounds.x + bounds.width / 2.0).max(1.0);
            let click_y = bounds.y + bounds.height / 2.0;
            let hit = layout.hit_test(click_x, click_y);
            assert!(
                matches!(hit, TreeViewHit::Row(idx) if idx == vis.row_idx),
                "row {} center ({:.1}, {:.1}) hit_test returned {:?}; expected Row({})",
                vis.row_idx,
                click_x,
                click_y,
                hit,
                vis.row_idx
            );

            // Plus a sanity pixel check: the row's interior has paint
            // (header bg or row text). Row pitch agreement → the
            // expected y-band of each row is non-empty.
            let y_top = (bounds.y + 1.0).floor() as i32;
            let y_bot = (bounds.y + bounds.height - 1.0).floor() as i32;
            let painted = first_painted_in(&data, stride, (1, W - 1), (y_top, y_bot.min(H)));
            assert!(
                painted.is_some(),
                "row {} interior y range {}..{} contained no paint — \
                 row-pitch drift suspected",
                vis.row_idx,
                y_top,
                y_bot
            );
        }
    }

    // ── Inline editing paint test ───────────────────────────────────

    use crate::primitives::tree::TreeRowEditState;

    #[test]
    fn gtk_editing_row_has_caret_pixels() {
        let tree = make_tree(vec![
            leaf(0, "alpha"),
            TreeRow {
                path: vec![1],
                indent: 0,
                icon: None,
                text: StyledText::plain("old-name".to_string()),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit: Some(TreeRowEditState {
                    text: "new-name".into(),
                    cursor: 3, // after "new"
                    selection_anchor: None,
                    placeholder: None,
                }),
            },
            leaf(2, "gamma"),
        ]);
        let (mut surface, layout) = paint_then_layout(&tree);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        // The editing row (index 1) should have non-background pixels
        // (the caret bar) painted in its interior.
        let vis = &layout.visible_rows[1];
        let bounds = vis.bounds;
        let y_top = (bounds.y + 1.0).floor() as i32;
        let y_bot = (bounds.y + bounds.height - 1.0).floor() as i32;
        let painted = first_painted_in(&data, stride, (1, W - 1), (y_top, y_bot.min(H)));
        assert!(
            painted.is_some(),
            "editing row interior should contain painted pixels (caret + text)"
        );
    }

    /// An inline-rename cursor/selection byte offset landing
    /// mid-multibyte-character must snap to the nearest char boundary
    /// rather than panic on slice operations like `&edit.text[..cursor]`.
    /// Paints a row with a multibyte name and a cursor position that sits
    /// inside a multibyte character's byte range, plus an active selection
    /// whose bounds also land mid-character.
    #[test]
    fn gtk_editing_row_with_multibyte_cursor_does_not_panic() {
        // "café🎉中文" — cursor/selection offsets below are deliberately
        // *not* char-boundary aligned; snap_to_char_boundary must rescue
        // them before any `&text[..n]` slice happens.
        let text = "café🎉中文";
        // byte 4 is inside the 2-byte 'é' (starts at byte 3); byte 8 is
        // inside the 4-byte emoji (starts at byte 6).
        let mid_char_cursor = 4;
        let mid_char_anchor = 8;
        assert!(!text.is_char_boundary(mid_char_cursor));
        assert!(!text.is_char_boundary(mid_char_anchor));

        let tree = make_tree(vec![
            leaf(0, "alpha"),
            TreeRow {
                path: vec![1],
                indent: 0,
                icon: None,
                text: StyledText::plain("old-name".to_string()),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit: Some(TreeRowEditState {
                    text: text.to_string(),
                    cursor: mid_char_cursor,
                    selection_anchor: Some(mid_char_anchor),
                    placeholder: None,
                }),
            },
            leaf(2, "gamma"),
        ]);

        // Must not panic.
        let (mut surface, layout) = paint_then_layout(&tree);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let vis = &layout.visible_rows[1];
        let bounds = vis.bounds;
        let y_top = (bounds.y + 1.0).floor() as i32;
        let y_bot = (bounds.y + bounds.height - 1.0).floor() as i32;
        let painted = first_painted_in(&data, stride, (1, W - 1), (y_top, y_bot.min(H)));
        assert!(
            painted.is_some(),
            "editing row with multibyte text should still paint (selection + caret + text)"
        );
    }

    // ── Chevron hit-test tests ──────────────────────────────────────

    fn branch(idx: u16, label: &str, expanded: bool) -> TreeRow {
        TreeRow {
            path: vec![idx],
            indent: 0,
            icon: None,
            text: StyledText::plain(label.to_string()),
            badge: None,
            is_expanded: Some(expanded),
            decoration: Decoration::Normal,
            edit: None,
        }
    }

    /// A click very close to the left edge of a branch row lands on the
    /// Chevron region (x < chevron_end_x).
    #[test]
    fn gtk_chevron_click_returns_chevron_hit() {
        let tree = make_tree(vec![branch(0, "src", true), leaf(1, "beta")]);
        let area = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = gtk_tree_layout(&tree, area, LINE_HEIGHT);
        // At indent=0 with default TreeStyle, chevron_end_x > 2.0.
        // Clicking at x=1.0 (well inside the chevron region) must give Chevron(0).
        let hit = layout.hit_test(1.0, layout.visible_rows[0].bounds.y + 1.0);
        assert!(
            matches!(hit, TreeViewHit::Chevron(0)),
            "x=1 on GTK branch row 0 should return Chevron(0), got {:?}",
            hit
        );
    }

    /// A click far to the right of a branch row lands on the Row region.
    #[test]
    fn gtk_body_click_returns_row_hit() {
        let tree = make_tree(vec![branch(0, "src", true)]);
        let area = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = gtk_tree_layout(&tree, area, LINE_HEIGHT);
        // The chevron is a small fraction of the row width. x=100 is safely past it.
        let hit = layout.hit_test(100.0, layout.visible_rows[0].bounds.y + 1.0);
        assert!(
            matches!(hit, TreeViewHit::Row(0)),
            "x=100 past chevron should return Row(0), got {:?}",
            hit
        );
    }

    /// Leaf rows always return Row regardless of x position.
    #[test]
    fn gtk_leaf_click_always_row_hit() {
        let tree = make_tree(vec![leaf(0, "main.rs")]);
        let area = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = gtk_tree_layout(&tree, area, LINE_HEIGHT);
        for x in [0.5_f32, 1.0, 5.0, 50.0] {
            let hit = layout.hit_test(x, layout.visible_rows[0].bounds.y + 1.0);
            assert!(
                matches!(hit, TreeViewHit::Row(0)),
                "leaf click at x={x} should return Row(0), got {:?}",
                hit
            );
        }
    }

    /// `gtk_tree_layout_measured` must use the real Pango-measured
    /// chevron glyph width, not `gtk_tree_layout`'s `line_height * 0.65`
    /// estimate — mirrors `macos::tree`'s
    /// `mac_tree_layout_uses_real_chevron_glyph_width_not_the_estimate`.
    #[test]
    fn gtk_tree_layout_measured_uses_real_chevron_glyph_width_not_the_estimate() {
        let tree = make_tree(vec![branch(0, "alpha", true)]);
        let area = QRect::new(0.0, 0.0, W as f32, H as f32);

        let surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new");
        let pango_layout = pangocairo::functions::create_layout(&cr);

        let estimated = gtk_tree_layout(&tree, area, LINE_HEIGHT);
        let measured = gtk_tree_layout_measured(&tree, area, LINE_HEIGHT, Some(&pango_layout), 0.0);

        // The Chevron hit-region's width *is* the row's chevron_end_x
        // (see `TreeView::layout`'s hit-region split).
        let est_x = estimated.hit_regions[0].0.width;
        let measured_x = measured.hit_regions[0].0.width;
        assert_ne!(
            est_x, measured_x,
            "measured chevron end-x must use the real Pango glyph width ({measured_x}), \
             not the line_height * 0.65 estimate ({est_x})",
        );
        // Exact geometry: 2px left margin + indent (0 here, `branch`'s
        // `indent` field) * round(line_height * 0.9) + the real glyph
        // advance + 4px gap. The default `chevron_expanded` is swapped for
        // the codicon `CHEVRON_DOWN` glyph before painting/measuring (see
        // `tree::tree_chevron_glyph`), so that is what must be measured here.
        pango_layout.set_text(&crate::primitives::tree::tree_chevron_glyph(
            &tree.style,
            true,
        ));
        let glyph_w = pango_layout.pixel_size().0.max(0) as f32;
        let indent_px = (LINE_HEIGHT as f32 * 0.9).round();
        let expected = 2.0 + tree.rows[0].indent as f32 * indent_px + glyph_w + 4.0;
        assert!(
            (measured_x - expected).abs() < 0.01,
            "measured chevron end-x {measured_x} must equal the Pango-measured \
             geometry {expected} (glyph advance {glyph_w})",
        );
        assert_eq!(
            measured.hit_test(measured_x - 1.0, 4.0),
            TreeViewHit::Chevron(0),
            "the measured boundary itself must still resolve to the chevron hit region",
        );
    }

    /// With `row_height` set, the painted row pitch is identical at two
    /// wildly different editor `line_height`s. Without the override,
    /// `item_height` scales with `line_height` and this would fail
    /// (`14.0 * 1.4 = 19.6` vs `48.0 * 1.4 = 67.2`).
    #[test]
    fn gtk_row_height_override_pins_pitch_across_line_heights() {
        let mut tree = make_tree(vec![
            leaf(0, "alpha"),
            leaf(1, "beta"),
            leaf(2, "gamma"),
            leaf(3, "delta"),
        ]);
        tree.style.row_height = Some(30);
        let area = QRect::new(0.0, 0.0, W as f32, H as f32);

        let small_font_layout = gtk_tree_layout(&tree, area, 14.0);
        let large_font_layout = gtk_tree_layout(&tree, area, 48.0);

        assert_eq!(
            small_font_layout.visible_rows.len(),
            large_font_layout.visible_rows.len(),
            "same tree at two line_heights must show the same row count"
        );
        for (small, large) in small_font_layout
            .visible_rows
            .iter()
            .zip(large_font_layout.visible_rows.iter())
        {
            assert_eq!(
                small.bounds, large.bounds,
                "row_height override must pin every row's bounds regardless \
                 of editor line_height (row {})",
                small.row_idx
            );
            assert_eq!(
                small.bounds.height, 30.0,
                "row height must equal the override"
            );
        }
    }

    /// Companion to the pitch-pinning test: a click at row N's painted
    /// position still resolves to row N at both line_heights, and
    /// `draw_tree`'s own internal layout (not just `gtk_tree_layout`
    /// called standalone) honours the same override — paint and
    /// hit-test can't drift apart by construction.
    #[test]
    fn gtk_row_height_override_click_resolves_to_same_row_at_any_line_height() {
        let mut tree = make_tree(vec![
            leaf(0, "alpha"),
            leaf(1, "beta"),
            leaf(2, "gamma"),
            leaf(3, "delta"),
        ]);
        tree.style.row_height = Some(22);

        for line_height in [14.0_f64, 48.0] {
            let surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
            let layout = {
                let cr = Context::new(&surface).expect("Context::new");
                cr.set_source_rgb(1.0, 1.0, 1.0);
                cr.paint().ok();
                let pango_layout = pangocairo::functions::create_layout(&cr);
                draw_tree(
                    &cr,
                    &pango_layout,
                    0.0,
                    0.0,
                    W as f64,
                    H as f64,
                    &tree,
                    &test_theme(),
                    line_height,
                    /* nerd_fonts */ false,
                );
                let area = QRect::new(0.0, 0.0, W as f32, H as f32);
                gtk_tree_layout(&tree, area, line_height)
            };

            for row_idx in 0..tree.rows.len() {
                let bounds = layout.visible_rows[row_idx].bounds;
                let hit = layout.hit_test(bounds.x + 5.0, bounds.y + bounds.height / 2.0);
                assert!(
                    matches!(hit, TreeViewHit::Row(idx) if idx == row_idx),
                    "at line_height={line_height}, row {row_idx} bounds {:?} should hit_test \
                     back to Row({row_idx}), got {:?}",
                    bounds,
                    hit
                );
                assert_eq!(
                    bounds.height, 22.0,
                    "row {row_idx} must keep the overridden height at line_height={line_height}"
                );
            }
        }
    }

    /// `tree_layout` returns bounds relative to `area`'s origin
    /// regardless of where `area` itself lives — a contract that
    /// shared implementation [`crate::primitives::layout_metrics::tree_layout`]
    /// maintains across backends. Since the GTK and macOS backends share this
    /// implementation, this test verifies non-zero-origin coverage.
    #[test]
    fn layout_returns_local_coords_when_area_offset() {
        let tree = make_tree(vec![leaf(0, "alpha"), leaf(1, "beta"), leaf(2, "gamma")]);
        let area = QRect::new(0.0, 60.0, 240.0, 180.0);
        let layout = gtk_tree_layout(&tree, area, 16.0);

        let first = &layout.visible_rows[0];
        assert_eq!(
            first.bounds.y, 0.0,
            "visible_rows.bounds.y must be local (0.0), got {}",
            first.bounds.y,
        );

        for vr in &layout.visible_rows {
            let abs_x = area.x + vr.bounds.x + vr.bounds.width * 0.5;
            let abs_y = area.y + vr.bounds.y + vr.bounds.height * 0.5;
            // Localise the way a host must (subtract area.x/area.y)
            // before calling hit_test, per the documented contract.
            let hit = layout.hit_test(abs_x - area.x, abs_y - area.y);
            assert!(
                matches!(hit, TreeViewHit::Row(idx) if idx == vr.row_idx),
                "row {} at local bounds {:?} did not round-trip through localised hit_test",
                vr.row_idx,
                vr.bounds,
            );
        }
    }

    /// A row with `Icon::color` set paints its icon glyph in that
    /// colour; a row without one paints it in the default row fg.
    /// Rows have no label text (so the icon glyph is the *only* ink in
    /// the row) and use a high-contrast theme (near-black default fg vs.
    /// white bg vs. a distinct orange icon colour), so the strongest
    /// departure from the white background within the row's bounds is
    /// unambiguously the icon glyph, not an antialiasing fringe pixel or
    /// competing label text.
    #[test]
    fn icon_color_paints_icon_glyph_in_that_color_else_default_fg() {
        use crate::types::Icon;

        let icon_color = Color::rgb(220, 80, 20);
        let default_fg = Color::rgb(10, 10, 10);
        let bg = Color::rgb(255, 255, 255);
        let theme = Theme {
            tab_bar_bg: bg,
            background: bg,
            foreground: default_fg,
            ..Theme::default()
        };

        let mut colored_row = leaf(0, "");
        colored_row.icon = Some(Icon::new("R", "R").with_color(icon_color));
        let mut plain_row = leaf(1, "");
        plain_row.icon = Some(Icon::new("R", "R"));
        let tree = make_tree(vec![colored_row, plain_row]);

        let mut surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().ok();
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_tree(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                W as f64,
                H as f64,
                &tree,
                &theme,
                LINE_HEIGHT,
                /* nerd_fonts */ false,
            );
        }
        let area = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = gtk_tree_layout(&tree, area, LINE_HEIGHT);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let dist = |a: (u8, u8, u8), c: Color| {
            let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
            d(a.0, c.r) + d(a.1, c.g) + d(a.2, c.b)
        };

        for (vis, expect_colored) in layout.visible_rows.iter().zip([true, false]) {
            let bounds = vis.bounds;
            let x_range = (
                (bounds.x + 1.0).floor() as i32,
                (bounds.x + bounds.width - 1.0).floor() as i32,
            );
            let y_range = (
                (bounds.y + 1.0).floor() as i32,
                (bounds.y + bounds.height - 1.0).floor() as i32,
            );
            // Most-inked pixel: the one furthest (squared RGB distance)
            // from the white background — robust against picking a
            // barely-antialiased fringe pixel near pure white.
            let mut best: Option<(u8, u8, u8)> = None;
            let mut best_d = -1i32;
            for y in y_range.0..y_range.1 {
                for x in x_range.0..x_range.1 {
                    if x < 0 || y < 0 || x >= W || y >= H {
                        continue;
                    }
                    let p = pixel(&data, stride, x, y);
                    let d = dist(p, bg);
                    if d > best_d {
                        best_d = d;
                        best = Some(p);
                    }
                }
            }
            let color = best.unwrap_or_else(|| {
                panic!(
                    "row {} contained no painted pixel — icon glyph missing",
                    vis.row_idx
                )
            });
            let closer_to_icon_color = dist(color, icon_color) < dist(color, default_fg);
            assert_eq!(
                closer_to_icon_color, expect_colored,
                "row {}: most-inked pixel {color:?} — expected closer to \
                 Icon::color {icon_color:?} than default fg {default_fg:?}: {expect_colored}",
                vis.row_idx,
            );
        }
    }

    /// The vertical scrollbar track is painted when the tree overflows,
    /// alongside row content. This test paints through `draw_tree` into
    /// a Cairo `ImageSurface` and probes a pixel inside the resolved
    /// track rect to confirm the track is rendered.
    #[test]
    fn gtk_paints_vertical_scrollbar_track_when_overflowing() {
        let tree = make_tree((0..40).map(|i| leaf(i, &format!("row{i}"))).collect());
        let area = QRect::new(0.0, 0.0, W as f32, H as f32);
        let item_height =
            crate::primitives::layout_metrics::tree_row_pitch(&tree, LINE_HEIGHT) as f32;
        let expected = tree
            .vscrollbar_with_track_width(
                area,
                item_height,
                crate::primitives::layout_metrics::pixel::TREE_VSCROLLBAR_WIDTH,
            )
            .expect("40 rows in a 200px viewport must need a v-scrollbar");

        let mut surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().ok();
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_tree(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                W as f64,
                H as f64,
                &tree,
                &test_theme(),
                LINE_HEIGHT,
                false,
            );
        }
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let probe_x = (expected.track.x + expected.track.width / 2.0).round() as i32;
        let probe_y = (expected.track.y + expected.track.height / 2.0).round() as i32;
        let (r, g, b) = pixel(
            &data,
            stride,
            probe_x.clamp(0, W - 1),
            probe_y.clamp(0, H - 1),
        );
        assert_ne!(
            (r, g, b),
            (255, 255, 255),
            "expected the v-scrollbar track at ({probe_x}, {probe_y}) to be painted \
             (non-background), got white — the #1075 regression this test guards"
        );
    }

    /// Before the `native_surface_paint` migration,
    /// `paint`'s row-being-edited path did not yet exist on Windows at
    /// all (`win::tree::draw_tree` painted the row's stale label
    /// instead — see that module's pre-migration doc). GTK already had
    /// full caret/selection support; this test just re-confirms GTK's
    /// caret still paints post-migration (companion to the primitive-
    /// level `paints_caret_for_row_being_edited` test, which covers Win
    /// and macOS via the shared `RecordingSurface`).
    #[test]
    fn gtk_editing_row_still_paints_caret_after_migration() {
        let tree = make_tree(vec![TreeRow {
            path: vec![0],
            indent: 0,
            icon: None,
            text: StyledText::plain("old-name".to_string()),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: Some(crate::primitives::tree::TreeRowEditState {
                text: "new-name".into(),
                cursor: 3,
                selection_anchor: None,
                placeholder: None,
            }),
        }]);
        let (mut surface, layout) = paint_then_layout(&tree);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let vis = &layout.visible_rows[0];
        let bounds = vis.bounds;
        let y_top = (bounds.y + 1.0).floor() as i32;
        let y_bot = (bounds.y + bounds.height - 1.0).floor() as i32;
        let painted = first_painted_in(&data, stride, (1, W - 1), (y_top, y_bot.min(H)));
        assert!(
            painted.is_some(),
            "editing row interior should still contain painted pixels (caret + text) \
             after the #1075 migration"
        );
    }
}
