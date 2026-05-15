//! macOS rasteriser for [`crate::TreeView`].
//!
//! Mirrors [`crate::gtk::tree::draw_tree`]: header rows use
//! `(line_height * 1.2)` pitch, leaves and branches use
//! `(line_height * 1.4)`. Chevron / icon / text / badge layout within
//! a row matches the GTK convention so a paired `macos_multi_tree`
//! example reads identically to its GTK twin.
//!
//! ## Scope omissions (follow-up)
//!
//! - **Inline `TreeRowEditState` text input** — caret, selection
//!   anchor, placeholder. Painted as a plain text fallback for now;
//!   full inline-edit input lands with the unified text-attribute pass
//!   alongside the editor selection highlight.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::event::Rect as QRect;
use crate::primitives::tree::{TreeRowMeasure, TreeView, TreeViewLayout};
use crate::theme::Theme;
use crate::types::{Color, Decoration};

/// Compute the layout the macOS rasteriser would produce for `tree`
/// in `area` at `line_height`. Hosts and tests call this to drive
/// hit-testing without re-deriving row pitch. Header rows use
/// `(line_height * 1.2).round()`, others use `(line_height * 1.4)`.
///
/// Coordinate frame: `visible_rows.bounds` and `hit_regions` are in
/// **tree-local** coords (origin at 0, 0), matching `tui_tree_layout`
/// and `gtk_tree_layout`. Hosts must subtract `area.x`/`area.y` from
/// absolute click coords before calling [`TreeViewLayout::hit_test`]
/// (the `tree_controller` compose helper and example AppLogic both
/// follow this convention).
pub fn mac_tree_layout(tree: &TreeView, area: QRect, line_height: f64) -> TreeViewLayout {
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

/// Draw a [`TreeView`] into `(x, y, w, h)` on `ctx`. Returns the
/// same layout `mac_tree_layout` would produce.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_tree(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    tree: &TreeView,
    theme: &Theme,
    line_height: f64,
) -> TreeViewLayout {
    let area = QRect::new(x as f32, y as f32, w as f32, h as f32);
    if w <= 0.0 || h <= 0.0 {
        return mac_tree_layout(tree, area, line_height);
    }

    let layout = mac_tree_layout(tree, area, line_height);

    CGContextSaveGState(ctx);
    CGContextClipToRect(ctx, CGRect::new_xywh(x, y, w, h));

    fill_rect(ctx, x, y, w, h, theme.tab_bar_bg);

    let indent_px = (line_height * 0.9).round();
    let header_height = (line_height * 1.2).round();
    let item_height = (line_height * 1.4).round();

    for vis_row in &layout.visible_rows {
        let row = &tree.rows[vis_row.row_idx];
        // Layout returns local coords; shift to absolute for paint.
        let row_x = vis_row.bounds.x as f64 + x;
        let row_y = vis_row.bounds.y as f64 + y;
        let row_w = vis_row.bounds.width as f64;
        let row_h = vis_row.bounds.height as f64;

        let is_header = matches!(row.decoration, Decoration::Header);
        let full_h = if is_header {
            header_height
        } else {
            item_height
        };
        // Skip rows the viewport clipped to a partial height — same
        // smoothing as GTK.
        if row_h < full_h - 0.5 {
            continue;
        }

        let path_selected = tree.selected_path.as_ref().is_some_and(|p| p == &row.path);
        let is_selected = tree.has_focus && path_selected;
        let is_inactive_selected = !tree.has_focus && path_selected;

        let (def_fg, row_bg) = if is_selected {
            (theme.header_fg, theme.selected_bg)
        } else if is_inactive_selected {
            (theme.foreground, theme.inactive_selected_bg)
        } else if is_header {
            (theme.header_fg, theme.header_bg)
        } else if matches!(row.decoration, Decoration::Muted) {
            (theme.muted_fg, theme.tab_bar_bg)
        } else {
            (theme.foreground, theme.tab_bar_bg)
        };

        fill_rect(ctx, row_x, row_y, row_w, row_h, row_bg);

        let mut cursor_x = row_x + 2.0 + (row.indent as f64) * indent_px;

        if let Some(expanded) = row.is_expanded {
            if tree.style.show_chevrons {
                let chevron = if expanded {
                    &tree.style.chevron_expanded
                } else {
                    &tree.style.chevron_collapsed
                };
                let (cw, ch) = measure_text(font, chevron);
                draw_text(
                    ctx,
                    font,
                    chevron,
                    cursor_x,
                    (row_y + (row_h - ch) / 2.0).round(),
                    color_to_cg(def_fg),
                );
                cursor_x += cw + 4.0;
            }
        } else {
            cursor_x += line_height * 0.8;
        }

        // Icon — uses the ASCII `fallback` (Nerd Font handling lives
        // at a higher layer; this rasteriser doesn't take a per-frame
        // toggle yet).
        if let Some(ref icon) = row.icon {
            let glyph = icon.fallback.as_str();
            let (iw, ih) = measure_text(font, glyph);
            draw_text(
                ctx,
                font,
                glyph,
                cursor_x,
                (row_y + (row_h - ih) / 2.0).round(),
                color_to_cg(def_fg),
            );
            cursor_x += iw + 6.0;
        }

        if let Some(ref edit) = row.edit {
            // Inline-edit fallback: render the text + placeholder
            // unstyled. Caret/selection painting deferred — see module
            // header.
            let render_text = if edit.text.is_empty() {
                edit.placeholder.clone().unwrap_or_default()
            } else {
                edit.text.clone()
            };
            let (_, th) = measure_text(font, &render_text);
            draw_text(
                ctx,
                font,
                &render_text,
                cursor_x,
                (row_y + (row_h - th) / 2.0).round(),
                color_to_cg(if edit.text.is_empty() {
                    theme.muted_fg
                } else {
                    def_fg
                }),
            );
            continue;
        }

        let badge_info = row.badge.as_ref().map(|b| {
            let (bw, _) = measure_text(font, &b.text);
            let bfg = b.fg.unwrap_or(theme.muted_fg);
            let bbg = b.bg.unwrap_or(row_bg);
            (b.text.clone(), bw, bfg, bbg)
        });
        let badge_reserve = badge_info
            .as_ref()
            .map(|(_, bw, ..)| *bw + 8.0)
            .unwrap_or(0.0);
        let text_right_limit = row_x + row_w - badge_reserve - 4.0;

        for span in &row.text.spans {
            if cursor_x >= text_right_limit {
                break;
            }
            let span_fg = if let Some(c) = span.fg {
                c
            } else if matches!(row.decoration, Decoration::Muted) {
                theme.muted_fg
            } else {
                def_fg
            };
            if let Some(sbg) = span.bg {
                let (sw, _) = measure_text(font, &span.text);
                fill_rect(
                    ctx,
                    cursor_x,
                    row_y,
                    sw.min(text_right_limit - cursor_x),
                    row_h,
                    sbg,
                );
            }
            let (sw, sh) = measure_text(font, &span.text);
            draw_text(
                ctx,
                font,
                &span.text,
                cursor_x,
                (row_y + (row_h - sh) / 2.0).round(),
                color_to_cg(span_fg),
            );
            cursor_x += sw;
        }

        if let Some((btext, bw, bfg, bbg)) = badge_info {
            let bx = row_x + row_w - bw - 4.0;
            if bx > cursor_x {
                if bbg != row_bg {
                    fill_rect(ctx, bx - 2.0, row_y, bw + 4.0, row_h, bbg);
                }
                let (_, bh) = measure_text(font, &btext);
                draw_text(
                    ctx,
                    font,
                    &btext,
                    bx,
                    row_y + (row_h - bh) / 2.0,
                    color_to_cg(bfg),
                );
            }
        }
    }

    CGContextRestoreGState(ctx);
    layout
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    CGContextFillRect(ctx, CGRect::new_xywh(x, y, w, h));
}

trait CGRectExt {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self;
}
impl CGRectExt for CGRect {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self {
        use core_graphics::geometry::{CGPoint, CGSize};
        CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h))
    }
}

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextClipToRect(c: CGContextRef, rect: CGRect);
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::Viewport;
    use crate::primitives::tree::{TreeRow, TreeViewHit};
    use crate::types::{Color, SelectionMode, StyledText, TreeStyle, WidgetId};
    use crate::Backend;

    const W: u32 = 240;
    const H: u32 = 240;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn leaf(idx: u16, label: &str) -> TreeRow {
        TreeRow {
            path: vec![idx],
            indent: 0,
            icon: None,
            text: StyledText::plain(label),
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
            text: StyledText::plain(label),
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

    fn paint_via_backend(tree: &TreeView) -> (BitmapSurface, TreeViewLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        // Match GTK convention: use background as the tree bg so probes
        // distinguish painted-by-tree from cleared-buffer pixels.
        backend.set_current_theme(Theme {
            tab_bar_bg: Color::rgb(255, 255, 255),
            background: Color::rgb(255, 255, 255),
            ..Theme::default()
        });
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_tree(QRect::new(0.0, 0.0, W as f32, H as f32), tree);
            let l = super::mac_tree_layout(
                tree,
                QRect::new(0.0, 0.0, W as f32, H as f32),
                b.line_height() as f64,
            );
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn header_row_paints_header_bg() {
        // Header row + plain leaf: probe inside the header row's
        // bounds and assert header_bg.
        let tree = make_tree(vec![header_row(0, "SECTION"), leaf(1, "alpha")]);
        let (surface, layout) = paint_via_backend(&tree);
        let hdr = &layout.visible_rows[0];
        let theme = Theme {
            tab_bar_bg: Color::rgb(255, 255, 255),
            background: Color::rgb(255, 255, 255),
            ..Theme::default()
        };
        // Probe near the right edge so chevron / text glyphs are out
        // of the way.
        let px = (hdr.bounds.x + hdr.bounds.width - 2.0) as u32;
        let py = (hdr.bounds.y + hdr.bounds.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b),
        );
    }

    #[test]
    fn selected_row_paints_selected_bg() {
        let mut tree = make_tree(vec![leaf(0, "alpha"), leaf(1, "beta"), leaf(2, "gamma")]);
        tree.selected_path = Some(vec![1]);
        let (surface, layout) = paint_via_backend(&tree);
        let theme = Theme::default();
        let sel = &layout.visible_rows[1];
        let px = (sel.bounds.x + sel.bounds.width - 2.0) as u32;
        let py = (sel.bounds.y + sel.bounds.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
        );
    }

    #[test]
    fn hit_test_resolves_each_visible_row() {
        let tree = make_tree(vec![
            leaf(0, "alpha"),
            leaf(1, "beta"),
            leaf(2, "gamma"),
            leaf(3, "delta"),
        ]);
        let (_surface, layout) = paint_via_backend(&tree);
        for vr in &layout.visible_rows {
            let cx = vr.bounds.x + vr.bounds.width * 0.5;
            let cy = vr.bounds.y + vr.bounds.height * 0.5;
            assert_eq!(
                layout.hit_test(cx, cy),
                TreeViewHit::Row(vr.row_idx),
                "row {} mid-point hit-test",
                vr.row_idx,
            );
        }
    }

    #[test]
    fn scroll_offset_shifts_visible_window() {
        let mut tree = make_tree((0..30).map(|i| leaf(i, &format!("item-{}", i))).collect());
        tree.scroll_offset = 5;
        let (_surface, layout) = paint_via_backend(&tree);
        let first = layout.visible_rows.first().expect("rows visible");
        assert_eq!(
            first.row_idx, 5,
            "first painted row should match scroll_offset",
        );
        // Hit-test at the top of the viewport must return row 5,
        // not row 0 — catches scroll-vs-paint drift.
        assert_eq!(
            layout.hit_test(10.0, first.bounds.y + 2.0),
            TreeViewHit::Row(5),
        );
    }

    #[test]
    fn layout_returns_local_coords_when_area_offset() {
        // Cross-backend contract: hit_regions and visible_rows.bounds
        // are in tree-local coords (origin 0, 0), regardless of where
        // `area` lives. Hosts (compose helpers, AppLogic) subtract
        // area.x/area.y from absolute click coords before hit_test.
        // This matches `tui_tree_layout` and `gtk_tree_layout`.
        //
        // Regression for #44 search-panel click drift: prior to the
        // fix, mac_tree_layout shifted hit_regions to absolute coords,
        // causing AppLogic that localised position (per the documented
        // contract) to hit the row at `position.y - 2*area.y` instead
        // of the row under the cursor.
        let tree = make_tree(vec![leaf(0, "alpha"), leaf(1, "beta"), leaf(2, "gamma")]);
        // Area offset by (0, 60) — typical when a tree lives below an
        // MSV header + aux input.
        let area = QRect::new(0.0, 60.0, 240.0, 180.0);
        let layout = mac_tree_layout(&tree, area, 16.0);
        // Locality: first row's bounds.y must be 0, not 60.
        let first = &layout.visible_rows[0];
        assert_eq!(
            first.bounds.y, 0.0,
            "visible_rows.bounds.y must be local (0.0), got {}",
            first.bounds.y,
        );
        // Round-trip: paint geometry → click resolution.
        // Simulate a click at the absolute centre of each painted row,
        // localise the way the AppLogic does, and assert it hits the
        // right row. Pre-fix this returned the row N positions earlier.
        for vr in &layout.visible_rows {
            let abs_x = area.x + vr.bounds.x + vr.bounds.width * 0.5;
            let abs_y = area.y + vr.bounds.y + vr.bounds.height * 0.5;
            let local_x = abs_x - area.x;
            let local_y = abs_y - area.y;
            assert_eq!(
                layout.hit_test(local_x, local_y),
                TreeViewHit::Row(vr.row_idx),
                "row {} click → wrong hit (coord-frame drift)",
                vr.row_idx,
            );
        }
    }

    #[test]
    fn mixed_header_and_leaves_use_different_row_pitch() {
        // Sanity: header_height < item_height (1.2 vs 1.4 multiplier).
        let tree = make_tree(vec![
            header_row(0, "SECTION"),
            leaf(1, "alpha"),
            leaf(2, "beta"),
        ]);
        let (_surface, layout) = paint_via_backend(&tree);
        let hdr_h = layout.visible_rows[0].bounds.height;
        let item_h = layout.visible_rows[1].bounds.height;
        assert!(
            hdr_h < item_h,
            "header pitch {} should be shorter than leaf pitch {}",
            hdr_h,
            item_h,
        );
    }
}
