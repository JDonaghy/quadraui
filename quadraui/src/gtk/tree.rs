//! GTK rasteriser for [`crate::TreeView`].
//!
//! Paints the tree onto a [`Context`] using a [`pango::Layout`] for
//! text measurement. Per-row heights are **non-uniform**: header rows
//! use `line_height`, leaves and ordinary branches use
//! `(line_height * 1.4).round()` (the established GTK convention).
//! The primitive's `tree.layout()` measurer reports each row's
//! height so the visible-row positions stack accurately.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::cairo_rgb;
use crate::event::Rect as QRect;
use crate::primitives::tree::{TreeRowEditState, TreeRowMeasure, TreeView, TreeViewLayout};
use crate::theme::Theme;
use crate::types::Decoration;

/// Compute the layout the GTK rasteriser would produce for `tree` in
/// `area` at `line_height`. Hosts and tests call this to drive
/// hit-testing without re-deriving row pitch (`1.0 × line_height` for
/// `Decoration::Header`, `1.4 × line_height` for everything else).
/// `draw_tree` uses this same helper internally so paint and hit_test
/// consume one layout instance per frame — the source-of-truth
/// contract `TreeView` exists to enforce.
///
/// Mirrors TUI's [`crate::tui::tui_tree_layout`] in spirit; differs
/// in row pitch (TUI = 1 cell uniform; GTK = mixed via decoration).
pub fn gtk_tree_layout(tree: &TreeView, area: QRect, line_height: f64) -> TreeViewLayout {
    let header_height = (line_height * 1.2).round();
    let item_height = (line_height * 1.4).round();
    tree.layout(area.width, area.height, |i| {
        let is_header = matches!(tree.rows[i].decoration, Decoration::Header);
        TreeRowMeasure::new(if is_header {
            header_height as f32
        } else {
            item_height as f32
        })
    })
}

/// Draw a [`TreeView`] into `(x, y, w, h)` on `cr`. `nerd_fonts_enabled`
/// controls which icon variant the consumer's icon registry exposes.
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
///   `tree.style.show_chevrons` is true; leaves get a `line_height *
///   0.8` leading offset for visual alignment.
/// - **Badge** (right-aligned): rendered in `badge.fg`/`badge.bg`
///   (falling back to [`Theme::muted_fg`] / row bg) when there's
///   room past the text.
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

    let bg = cairo_rgb(theme.tab_bar_bg);
    let hdr_bg = cairo_rgb(theme.header_bg);
    let hdr_fg = cairo_rgb(theme.header_fg);
    let fg = cairo_rgb(theme.foreground);
    let dim = cairo_rgb(theme.muted_fg);
    let sel = cairo_rgb(theme.selected_bg);
    let text_sel = cairo_rgb(theme.selection_bg);

    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(x, y, w, h);
    cr.fill().ok();

    layout.set_attributes(None);

    let indent_px = (line_height * 0.9).round();
    let header_height = (line_height * 1.2).round();
    let item_height = (line_height * 1.4).round();
    let tree_layout = gtk_tree_layout(tree, QRect::new(0.0, 0.0, w as f32, h as f32), line_height);

    for vis_row in &tree_layout.visible_rows {
        let row = &tree.rows[vis_row.row_idx];
        let row_y = (y + vis_row.bounds.y as f64).round();
        let row_h = vis_row.bounds.height as f64;

        // Skip rows the layout clipped to a partial height — painting
        // them produces a compressed background band at the section
        // boundary.
        let is_header = matches!(row.decoration, Decoration::Header);
        let full_h = if is_header {
            header_height
        } else {
            item_height
        };
        if row_h < full_h - 0.5 {
            continue;
        }

        let is_selected =
            tree.has_focus && tree.selected_path.as_ref().is_some_and(|p| p == &row.path);

        let (def_fg, row_bg) = if is_selected {
            (hdr_fg, sel)
        } else if is_header {
            (hdr_fg, hdr_bg)
        } else if matches!(row.decoration, Decoration::Muted) {
            (dim, bg)
        } else {
            (fg, bg)
        };

        cr.set_source_rgb(row_bg.0, row_bg.1, row_bg.2);
        cr.rectangle(x, row_y, w, row_h);
        cr.fill().ok();

        let mut cursor_x = x + 2.0 + (row.indent as f64) * indent_px;

        if let Some(expanded) = row.is_expanded {
            if tree.style.show_chevrons {
                let chevron = if expanded {
                    &tree.style.chevron_expanded
                } else {
                    &tree.style.chevron_collapsed
                };
                cr.set_source_rgb(def_fg.0, def_fg.1, def_fg.2);
                layout.set_text(chevron);
                let (cw, ch) = layout.pixel_size();
                cr.move_to(cursor_x, (row_y + (row_h - ch as f64) / 2.0).round());
                pcfn::show_layout(cr, layout);
                cursor_x += cw as f64 + 4.0;
            }
        } else {
            cursor_x += line_height * 0.8;
        }

        if let Some(ref icon) = row.icon {
            let glyph = if nerd_fonts_enabled {
                icon.glyph.as_str()
            } else {
                icon.fallback.as_str()
            };
            cr.set_source_rgb(def_fg.0, def_fg.1, def_fg.2);
            layout.set_text(glyph);
            let (iw, ih) = layout.pixel_size();
            cr.move_to(cursor_x, (row_y + (row_h - ih as f64) / 2.0).round());
            pcfn::show_layout(cr, layout);
            cursor_x += iw as f64 + 6.0;
        }

        if let Some(ref edit) = row.edit {
            paint_edit_input_gtk(
                cr,
                layout,
                cursor_x,
                row_y,
                row_h,
                x + w,
                edit,
                def_fg,
                text_sel,
                dim,
            );
        } else {
            let badge_info = row.badge.as_ref().map(|badge| {
                layout.set_text(&badge.text);
                let (bw, _) = layout.pixel_size();
                let bfg = badge.fg.map(cairo_rgb).unwrap_or(dim);
                let bbg = badge.bg.map(cairo_rgb).unwrap_or(row_bg);
                (badge.text.clone(), bw as f64, bfg, bbg)
            });
            let badge_reserve = badge_info
                .as_ref()
                .map(|(_, bw, ..)| *bw + 8.0)
                .unwrap_or(0.0);
            let text_right_limit = x + w - badge_reserve - 4.0;

            for span in &row.text.spans {
                if cursor_x >= text_right_limit {
                    break;
                }
                let span_fg = if let Some(c) = span.fg {
                    cairo_rgb(c)
                } else if matches!(row.decoration, Decoration::Muted) {
                    dim
                } else {
                    def_fg
                };
                if let Some(sbg) = span.bg {
                    let span_bg = cairo_rgb(sbg);
                    layout.set_text(&span.text);
                    let (sw, _) = layout.pixel_size();
                    cr.set_source_rgb(span_bg.0, span_bg.1, span_bg.2);
                    cr.rectangle(
                        cursor_x,
                        row_y,
                        (sw as f64).min(text_right_limit - cursor_x),
                        row_h,
                    );
                    cr.fill().ok();
                }
                cr.set_source_rgb(span_fg.0, span_fg.1, span_fg.2);
                layout.set_text(&span.text);
                let (sw, sh) = layout.pixel_size();
                cr.move_to(cursor_x, (row_y + (row_h - sh as f64) / 2.0).round());
                pcfn::show_layout(cr, layout);
                cursor_x += sw as f64;
            }

            if let Some((btext, bw, bfg, bbg)) = badge_info {
                let bx = x + w - bw - 4.0;
                if bx > cursor_x {
                    if bbg != row_bg {
                        cr.set_source_rgb(bbg.0, bbg.1, bbg.2);
                        cr.rectangle(bx - 2.0, row_y, bw + 4.0, row_h);
                        cr.fill().ok();
                    }
                    cr.set_source_rgb(bfg.0, bfg.1, bfg.2);
                    layout.set_text(&btext);
                    let (_, bh) = layout.pixel_size();
                    cr.move_to(bx, row_y + (row_h - bh as f64) / 2.0);
                    pcfn::show_layout(cr, layout);
                }
            }
        }
    }

    layout.set_attributes(None);
}

#[allow(clippy::too_many_arguments)]
fn paint_edit_input_gtk(
    cr: &Context,
    layout: &pango::Layout,
    text_x: f64,
    row_y: f64,
    row_h: f64,
    right_edge: f64,
    edit: &TreeRowEditState,
    fg: (f64, f64, f64),
    sel_rgb: (f64, f64, f64),
    dim: (f64, f64, f64),
) {
    let text_w = right_edge - text_x - 4.0;
    if text_w <= 0.0 {
        return;
    }

    if edit.text.is_empty() {
        if let Some(ref ph) = edit.placeholder {
            cr.set_source_rgb(dim.0, dim.1, dim.2);
            layout.set_text(ph);
            let (_, th) = layout.pixel_size();
            cr.move_to(text_x, (row_y + (row_h - th as f64) / 2.0).round());
            pcfn::show_layout(cr, layout);
        }
        // Caret at position 0.
        cr.set_source_rgb(fg.0, fg.1, fg.2);
        cr.rectangle(text_x, row_y + 3.0, 1.5, row_h - 6.0);
        cr.fill().ok();
        return;
    }

    // Selection highlight.
    if let Some(anchor) = edit.selection_anchor {
        if anchor != edit.cursor {
            let lo = anchor.min(edit.cursor).min(edit.text.len());
            let hi = anchor.max(edit.cursor).min(edit.text.len());
            let prefix = &edit.text[..lo];
            let sel_text = &edit.text[lo..hi];
            layout.set_text(prefix);
            let (prefix_w, _) = layout.pixel_size();
            layout.set_text(sel_text);
            let (sel_w, _) = layout.pixel_size();
            cr.set_source_rgb(sel_rgb.0, sel_rgb.1, sel_rgb.2);
            cr.rectangle(
                text_x + prefix_w as f64,
                row_y + 2.0,
                sel_w as f64,
                row_h - 4.0,
            );
            cr.fill().ok();
        }
    }

    // Text.
    cr.set_source_rgb(fg.0, fg.1, fg.2);
    layout.set_text(&edit.text);
    let (_, th) = layout.pixel_size();
    cr.move_to(text_x, (row_y + (row_h - th as f64) / 2.0).round());
    pcfn::show_layout(cr, layout);

    // Thin vertical caret bar.
    let cursor_byte = edit.cursor.min(edit.text.len());
    let cursor_prefix = &edit.text[..cursor_byte];
    layout.set_text(cursor_prefix);
    let (cx_off, _) = layout.pixel_size();
    let caret_x = text_x + cx_off as f64;
    cr.set_source_rgb(fg.0, fg.1, fg.2);
    cr.rectangle(caret_x, row_y + 3.0, 1.5, row_h - 6.0);
    cr.fill().ok();
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
// are 1.4×line_height stack inconsistently). GTK's `draw_tree` uses
// `gtk_tree_layout` for both paint and hit-test, so the contract is
// "they consume the same layout" — these tests verify that contract.
//
// Tests gated `#[cfg(all(test, feature = "gtk"))]` so they only run
// under `cargo test --features gtk`. They don't need a real display.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::tree::{TreeRow, TreeView, TreeViewHit};
    use crate::types::{Color, SelectionMode, StyledText, TreeStyle, WidgetId};
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
            scroll_offset: 0,
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
        tree.scroll_offset = 3;
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
}
