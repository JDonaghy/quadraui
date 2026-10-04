//! macOS rasteriser for [`crate::TreeView`].
//!
//! Content painting (background, rows, chevron/icon/badge/text or
//! inline-edit, vertical scrollbar) moved to the shared
//! [`crate::primitives::tree::native_surface_paint::paint`] (#1075,
//! `PaintSurface` Phase 4 slice 2/8) — see that fn's module doc for
//! what's shared. Notably, this migration upgrades macOS's inline-rename
//! rendering from a plain-text fallback (this module's pre-migration
//! "Scope omissions" — caret/selection were GTK-only) to the same full
//! caret + selection-highlight + placeholder treatment every backend now
//! shares.
//!
//! Header rows use `(line_height * 1.2)` pitch, leaves and branches use
//! `(line_height * 1.4)` unless `TreeStyle::row_height` overrides the
//! non-header pitch (#623). Chevron / icon / text / badge layout within
//! a row matches the GTK convention so a paired `macos_multi_tree`
//! example reads identically to its GTK twin.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::cg::{rect, CGContextClipToRect, CGContextRestoreGState, CGContextSaveGState};
use crate::event::Rect as QRect;
use crate::primitives::tree::{TreeView, TreeViewLayout};
use crate::theme::Theme;

/// Compute the layout the macOS rasteriser would produce for `tree`
/// in `area` at `line_height`. Hosts and tests call this to drive
/// hit-testing without re-deriving row pitch. Header rows use
/// `(line_height * 1.2).round()`, others use `(line_height * 1.4)`
/// unless `tree.style.row_height` overrides the non-header pitch (#623).
///
/// Coordinate frame: `visible_rows.bounds` and `hit_regions` are in
/// **tree-local** coords (origin at 0, 0), matching `tui_tree_layout`
/// and `gtk_tree_layout`. Hosts must subtract `area.x`/`area.y` from
/// absolute click coords before calling [`TreeViewLayout::hit_test`]
/// (the `tree_controller` compose helper and example AppLogic both
/// follow this convention).
///
/// Thin wrapper over [`crate::primitives::layout_metrics::tree_layout`]
/// (#499) — identical to [`crate::gtk::tree::gtk_tree_layout`], now
/// shared instead of duplicated.
pub fn mac_tree_layout(tree: &TreeView, area: QRect, line_height: f64) -> TreeViewLayout {
    crate::primitives::layout_metrics::tree_layout(tree, area, line_height)
}

/// Draw a [`TreeView`] into `(x, y, w, h)` on `ctx`. Returns the
/// same layout `mac_tree_layout` would produce.
///
/// `nerd_fonts_enabled` picks `row.icon.glyph` when `true` and
/// `row.icon.fallback` when `false` (issue #804) — pass
/// `MacBackend`'s own flag (set via [`crate::Backend::set_nerd_fonts`]),
/// same convention as `macos::activity_bar::draw_activity_bar` and the
/// TUI/GTK `draw_tree` rasterisers.
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
    nerd_fonts_enabled: bool,
) -> TreeViewLayout {
    let area = QRect::new(x as f32, y as f32, w as f32, h as f32);
    if w <= 0.0 || h <= 0.0 {
        return mac_tree_layout(tree, area, line_height);
    }

    let layout = mac_tree_layout(tree, area, line_height);

    CGContextSaveGState(ctx);
    CGContextClipToRect(ctx, rect(x, y, w, h));

    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    crate::primitives::tree::native_surface_paint::paint(
        tree,
        area,
        &layout,
        line_height as f32,
        nerd_fonts_enabled,
        &mut surface,
        theme,
    );

    CGContextRestoreGState(ctx);
    layout
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::Viewport;
    use crate::primitives::tree::{TreeRow, TreeViewHit};
    use crate::types::{Color, Decoration, SelectionMode, StyledText, TreeStyle, WidgetId};
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

    /// #623: with `row_height` set, the row pitch is pinned regardless
    /// of `line_height` — mirrors `gtk_row_height_override_pins_pitch_across_line_heights`.
    /// Without the override, item pitch scales with line_height and this
    /// would fail (`14.0 * 1.4 = 19.6` vs `48.0 * 1.4 = 67.2`).
    #[test]
    fn row_height_override_pins_pitch_across_line_heights() {
        let mut tree = make_tree(vec![
            leaf(0, "alpha"),
            leaf(1, "beta"),
            leaf(2, "gamma"),
            leaf(3, "delta"),
        ]);
        tree.style.row_height = Some(22);
        let area = QRect::new(0.0, 0.0, W as f32, H as f32);

        let small = mac_tree_layout(&tree, area, 14.0);
        let large = mac_tree_layout(&tree, area, 48.0);

        assert_eq!(small.visible_rows.len(), large.visible_rows.len());
        for (s, l) in small.visible_rows.iter().zip(large.visible_rows.iter()) {
            assert_eq!(
                s.bounds, l.bounds,
                "row_height override must pin row {} bounds across line_heights",
                s.row_idx
            );
            assert_eq!(s.bounds.height, 22.0);

            // Click at the row's center still resolves to the same row
            // at either line_height — paint and hit-test stay in sync.
            let cx = s.bounds.x + s.bounds.width * 0.5;
            let cy = s.bounds.y + s.bounds.height * 0.5;
            assert_eq!(small.hit_test(cx, cy), TreeViewHit::Row(s.row_idx));
            assert_eq!(large.hit_test(cx, cy), TreeViewHit::Row(s.row_idx));
        }
    }

    /// #804: `nerd_fonts_enabled` selects `row.icon.glyph` vs
    /// `row.icon.fallback` — previously a stored-but-ignored flag,
    /// `draw_tree` always painted `fallback` regardless of
    /// `MacBackend::nerd_fonts_enabled`. Uses two ASCII strings of
    /// clearly different width (`"WWWW"` vs `"E"`) rather than a real
    /// Nerd Font codepoint, so the assertion holds headless without a
    /// Nerd Font installed — same reasoning as
    /// `gtk::activity_bar::nerd_fonts_flag_selects_glyph_or_fallback`.
    /// Measures the painted icon's pixel bounding-box width via
    /// foreground-vs-background scanning — asserted on pixels, not
    /// screen text, per quadraui#555. This test was observed RED against
    /// the pre-fix `draw_tree` (both widths equal to the `"E"` fallback's).
    #[test]
    fn nerd_fonts_flag_selects_glyph_or_fallback() {
        use crate::types::Icon;

        let row = TreeRow {
            path: vec![0],
            indent: 0,
            icon: Some(Icon::new("WWWW", "E")),
            text: StyledText::plain(""),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: None,
        };
        let tree = make_tree(vec![row]);

        let painted_width = |nerd_fonts_enabled: bool| -> u32 {
            let surface = BitmapSurface::new(W, H);
            surface.fill(1.0, 1.0, 1.0, 1.0);
            let mut backend = MacBackend::new();
            backend.set_current_theme(Theme {
                tab_bar_bg: Color::rgb(255, 255, 255),
                background: Color::rgb(255, 255, 255),
                ..Theme::default()
            });
            backend.set_current_font(font());
            backend.set_nerd_fonts(nerd_fonts_enabled);
            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            backend.enter_frame_scope(surface.context_ptr(), |b| {
                b.draw_tree(QRect::new(0.0, 0.0, W as f32, H as f32), &tree);
            });
            backend.end_frame();

            // First (only) row's band is well within y in [0, 20); probe
            // its vertical mid-point.
            let mid_y = 10u32;
            let mut left = None;
            let mut right = None;
            for x in 0..W {
                let (r, g, b, _) = surface.pixel(x, mid_y);
                if (r, g, b) != (255, 255, 255) {
                    left.get_or_insert(x);
                    right = Some(x);
                }
            }
            match (left, right) {
                (Some(l), Some(r)) => r - l + 1,
                _ => 0,
            }
        };

        let glyph_width = painted_width(true);
        let fallback_width = painted_width(false);
        assert!(
            glyph_width > fallback_width,
            "nerd_fonts_enabled: true should paint the wider glyph icon \
             (\"WWWW\", measured {glyph_width}px) vs the narrower fallback \
             (\"E\", measured {fallback_width}px) painted when false"
        );
    }

    /// #1057: a row with `Icon::color` set paints its icon glyph in that
    /// colour; a row without one paints it in the default row fg,
    /// unchanged from pre-#1057 rendering. Empty row text (as in
    /// `nerd_fonts_flag_selects_glyph_or_fallback` above) means the icon
    /// glyph is the only ink in the row, so the most-inked pixel on the
    /// row's mid-line is unambiguously the icon.
    #[test]
    fn icon_color_paints_icon_glyph_in_that_color_else_default_fg() {
        use crate::types::Icon;

        let icon_color = Color::rgb(220, 80, 20);
        let default_fg = Color::rgb(10, 10, 10);
        let bg = Color::rgb(255, 255, 255);

        let dist = |a: (u8, u8, u8), c: Color| {
            let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
            d(a.0, c.r) + d(a.1, c.g) + d(a.2, c.b)
        };

        let most_inked_on_row = |icon: Icon| -> (u8, u8, u8) {
            let row = TreeRow {
                path: vec![0],
                indent: 0,
                icon: Some(icon),
                text: StyledText::plain(""),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit: None,
            };
            let tree = make_tree(vec![row]);

            let surface = BitmapSurface::new(W, H);
            surface.fill(1.0, 1.0, 1.0, 1.0);
            let mut backend = MacBackend::new();
            backend.set_current_theme(Theme {
                tab_bar_bg: bg,
                background: bg,
                foreground: default_fg,
                ..Theme::default()
            });
            backend.set_current_font(font());
            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            backend.enter_frame_scope(surface.context_ptr(), |b| {
                b.draw_tree(QRect::new(0.0, 0.0, W as f32, H as f32), &tree);
            });
            backend.end_frame();

            // First (only) row's band is well within y in [0, 20); probe
            // its vertical mid-point.
            let mid_y = 10u32;
            let mut best = (bg.r, bg.g, bg.b);
            let mut best_d = -1i32;
            for x in 0..W {
                let (r, g, b, _) = surface.pixel(x, mid_y);
                let c = (r, g, b);
                let d = dist(c, bg);
                if d > best_d {
                    best_d = d;
                    best = c;
                }
            }
            best
        };

        let colored = most_inked_on_row(Icon::new("R", "R").with_color(icon_color));
        assert!(
            dist(colored, icon_color) < dist(colored, default_fg),
            "row with Icon::color set: most-inked pixel {colored:?} should be \
             closer to Icon::color {icon_color:?} than default fg {default_fg:?}"
        );

        let uncolored = most_inked_on_row(Icon::new("R", "R"));
        assert!(
            dist(uncolored, default_fg) < dist(uncolored, icon_color),
            "row without Icon::color: most-inked pixel {uncolored:?} should be \
             closer to default fg {default_fg:?} than the unrelated colour \
             {icon_color:?}"
        );
    }

    /// #1075 regression: before the `native_surface_paint` migration,
    /// `draw_tree` never painted a vertical scrollbar even though
    /// `Backend::tree_vscrollbar` (#1043) already returned real
    /// geometry for hit-testing. This would have failed (background
    /// colour, nothing painted) against the pre-#1075 body.
    #[test]
    fn paints_vertical_scrollbar_track_when_overflowing() {
        let tree = make_tree((0..40).map(|i| leaf(i, &format!("row{i}"))).collect());
        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_theme(Theme {
            tab_bar_bg: Color::rgb(255, 255, 255),
            background: Color::rgb(255, 255, 255),
            ..Theme::default()
        });
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let area = QRect::new(0.0, 0.0, W as f32, H as f32);
        let expected = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_tree(area, &tree);
            let item_height =
                crate::primitives::layout_metrics::tree_row_pitch(&tree, b.line_height() as f64)
                    as f32;
            *expected.borrow_mut() = tree.vscrollbar(area, item_height);
        });
        backend.end_frame();
        let expected = expected
            .into_inner()
            .expect("40 rows should overflow this viewport");

        let probe_x = (expected.track.x + expected.track.width / 2.0) as u32;
        let probe_y = (expected.track.y + expected.track.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(probe_x.min(W - 1), probe_y.min(H - 1));
        assert_ne!(
            (r, g, b),
            (255, 255, 255),
            "expected the v-scrollbar track at ({probe_x}, {probe_y}) to be painted \
             (non-background) — the #1075 regression this test guards"
        );
    }

    /// #1075: macOS upgrades from a plain-text inline-rename fallback
    /// (pre-migration "Scope omissions") to the same caret/selection
    /// treatment GTK always had. A row mid-rename must still paint
    /// *something* distinct from the background inside its bounds.
    #[test]
    fn editing_row_paints_something_after_migration() {
        let mut rows = vec![leaf(0, "alpha"), leaf(1, "old-name"), leaf(2, "gamma")];
        rows[1].edit = Some(crate::primitives::tree::TreeRowEditState {
            text: "new-name".into(),
            cursor: 3,
            selection_anchor: None,
            placeholder: None,
        });
        let tree = make_tree(rows);
        let (surface, layout) = paint_via_backend(&tree);
        let bg = (255u8, 255u8, 255u8);

        let row1 = &layout.visible_rows[1];
        let y = (row1.bounds.y + row1.bounds.height / 2.0) as u32;
        let mut found_paint = false;
        for x in (row1.bounds.x as u32)..(row1.bounds.x + row1.bounds.width) as u32 {
            let (r, g, b, _) = surface.pixel(x.min(W - 1), y.min(H - 1));
            if (r, g, b) != bg {
                found_paint = true;
                break;
            }
        }
        assert!(
            found_paint,
            "editing row interior should contain painted pixels (caret + text)"
        );
    }
}
