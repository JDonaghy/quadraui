//! macOS rasteriser for [`crate::TreeView`].
//!
//! Content painting (background, rows, chevron/icon/badge/text or
//! inline-edit, vertical scrollbar) is shared with other backends via
//! [`crate::primitives::tree::native_surface_paint::paint`] — see that
//! fn's module doc for what's shared. This provides full caret +
//! selection-highlight + placeholder treatment for inline-rename across
//! all backends.
//!
//! Header rows use `(line_height * 1.2)` pitch, leaves and branches use
//! `(line_height * 1.4)` unless `TreeStyle::row_height` overrides the
//! non-header pitch. Chevron / icon / text / badge layout within
//! a row matches the GTK convention so a paired `macos_multi_tree`
//! example reads identically to its GTK twin.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::cg::{rect, CGContextClipToRect, CGContextRestoreGState, CGContextSaveGState};
use crate::event::Rect as QRect;
use crate::primitives::tree::{TreeView, TreeViewLayout};
use crate::theme::Theme;

/// Compute the layout the macOS rasteriser would produce for `tree`
/// in `area` at `line_height`. Header rows use `(line_height * 1.2).round()`,
/// others use `(line_height * 1.4)` unless `tree.style.row_height` overrides
/// the non-header pitch.
///
/// Coordinate frame: `visible_rows.bounds` and `hit_regions` are in
/// **tree-local** coords (origin at 0, 0), matching `tui_tree_layout`
/// and `gtk_tree_layout`. Hosts must subtract `area.x`/`area.y` from
/// absolute click coords before calling [`TreeViewLayout::hit_test`].
///
/// Thin wrapper over
/// [`crate::primitives::layout_metrics::tree_layout_measured`], the same
/// shared pixel-layout math [`crate::gtk::tree::gtk_tree_layout`] uses.
/// `font` measures each branch row's chevron glyph
/// (`TreeStyle::chevron_expanded`/`chevron_collapsed`) for real, rather
/// than the `line_height * 0.65` advance the unmeasured `tree_layout`
/// estimates — `CTFont` implements
/// [`crate::primitives::layout_metrics::TextMeasure`] directly, so no
/// adapter struct is needed (unlike GTK's `PangoTextMeasure`).
pub fn mac_tree_layout(
    tree: &TreeView,
    area: QRect,
    line_height: f64,
    font: &CTFont,
) -> TreeViewLayout {
    crate::primitives::layout_metrics::tree_layout_measured(tree, area, line_height, font)
}

/// Draw a [`TreeView`] into `(x, y, w, h)` on `ctx`. Returns the
/// same layout `mac_tree_layout` would produce.
///
/// `nerd_fonts_enabled` picks `row.icon.glyph` when `true` and
/// `row.icon.fallback` when `false` — pass
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
        return mac_tree_layout(tree, area, line_height, font);
    }

    let layout = mac_tree_layout(tree, area, line_height, font);

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

    fn branch_row(idx: u16, label: &str, expanded: bool) -> TreeRow {
        TreeRow {
            path: vec![idx],
            indent: 1,
            icon: None,
            text: StyledText::plain(label),
            badge: None,
            is_expanded: Some(expanded),
            decoration: Decoration::Normal,
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
                b.chrome_line_height() as f64,
                &font(),
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
        let tree = make_tree(vec![leaf(0, "alpha"), leaf(1, "beta"), leaf(2, "gamma")]);
        // Area offset by (0, 60) — typical when a tree lives below an
        // MSV header + aux input.
        let area = QRect::new(0.0, 60.0, 240.0, 180.0);
        let layout = mac_tree_layout(&tree, area, 16.0, &font());
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

    /// `mac_tree_layout` must measure the chevron glyph with the real
    /// `CTFont` instead of falling back to the unmeasured
    /// `line_height * 0.65` estimate `tree_layout` uses — mirrors
    /// `tree_layout_measured_uses_real_chevron_glyph_width_not_the_estimate`
    /// in `primitives::layout_metrics`, but exercised end to end through
    /// the macOS wrapper with a real `Menlo` `CTFont`.
    #[test]
    fn mac_tree_layout_uses_real_chevron_glyph_width_not_the_estimate() {
        let tree = make_tree(vec![branch_row(0, "alpha", true)]);
        let area = QRect::new(0.0, 0.0, W as f32, H as f32);

        let estimated = crate::primitives::layout_metrics::tree_layout(&tree, area, 16.0);
        let measured = mac_tree_layout(&tree, area, 16.0, &font());

        // The Chevron hit-region's width *is* the row's chevron_end_x
        // (see `TreeView::layout`'s hit-region split).
        let est_x = estimated.hit_regions[0].0.width;
        let measured_x = measured.hit_regions[0].0.width;
        assert_ne!(
            est_x, measured_x,
            "measured chevron end-x must use the real Menlo glyph width ({measured_x}), \
             not the line_height * 0.65 estimate ({est_x})",
        );
        // Exact geometry: 2px left margin + one indent level
        // (`round(line_height * 0.9)`) + the real glyph advance + 4px gap.
        // The default `chevron_expanded` is swapped for the codicon
        // `CHEVRON_DOWN` glyph before painting/measuring (see
        // `tree::tree_chevron_glyph`), so that is what must be measured here.
        let glyph_w = crate::primitives::layout_metrics::TextMeasure::width_of(
            &font(),
            &crate::primitives::tree::tree_chevron_glyph(&tree.style, true),
        );
        let expected = 2.0 + (16.0f32 * 0.9).round() + glyph_w + 4.0;
        assert!(
            (measured_x - expected).abs() < 0.01,
            "measured chevron end-x {measured_x} must equal the CTFont-measured \
             geometry {expected} (glyph advance {glyph_w})",
        );
        assert_eq!(
            measured.hit_test(measured_x - 1.0, 4.0),
            TreeViewHit::Chevron(0),
            "the measured boundary itself must still resolve to the chevron hit region",
        );
    }

    /// `TreeStyle::chevron_expanded` is painted verbatim through the shared
    /// `primitives::tree::native_surface_paint::paint` path `draw_tree`
    /// calls — this is the tree chevron's existing override mechanism (no
    /// new `Backend` trait method is needed, unlike the tab-bar / data-table
    /// / context-menu sites). An empty-text branch row isolates the chevron
    /// glyph as the row's only ink, so the painted ink span measures it
    /// directly: a wide override string paints wider than the narrow
    /// codicon substitute `TreeStyle::default()` paints.
    #[test]
    fn chevron_override_paints_app_glyph_wider_than_default_codicon() {
        let mut overridden = make_tree(vec![branch_row(0, "", true)]);
        overridden.style.chevron_expanded = "WWWWWWWW".to_string();
        let default_tree = make_tree(vec![branch_row(0, "", true)]);

        let ink_width = |tree: &TreeView| -> u32 {
            let (surface, layout) = paint_via_backend(tree);
            let row = &layout.visible_rows[0];
            let mid_y = (row.bounds.y + row.bounds.height / 2.0) as u32;
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

        let overridden_width = ink_width(&overridden);
        let default_width = ink_width(&default_tree);
        assert!(
            overridden_width > default_width,
            "the overridden chevron_expanded (\"WWWWWWWW\", measured \
             {overridden_width}px) should paint wider than the default \
             codicon substitute (measured {default_width}px)"
        );
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

    /// With `row_height` set, the row pitch is pinned regardless
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
        tree.style.row_height = Some(30);
        let area = QRect::new(0.0, 0.0, W as f32, H as f32);

        let small = mac_tree_layout(&tree, area, 14.0, &font());
        let large = mac_tree_layout(&tree, area, 48.0, &font());

        assert_eq!(small.visible_rows.len(), large.visible_rows.len());
        for (s, l) in small.visible_rows.iter().zip(large.visible_rows.iter()) {
            assert_eq!(
                s.bounds, l.bounds,
                "row_height override must pin row {} bounds across line_heights",
                s.row_idx
            );
            assert_eq!(s.bounds.height, 30.0);

            // Click at the row's center still resolves to the same row
            // at either line_height — paint and hit-test stay in sync.
            let cx = s.bounds.x + s.bounds.width * 0.5;
            let cy = s.bounds.y + s.bounds.height * 0.5;
            assert_eq!(small.hit_test(cx, cy), TreeViewHit::Row(s.row_idx));
            assert_eq!(large.hit_test(cx, cy), TreeViewHit::Row(s.row_idx));
        }
    }

    /// `nerd_fonts_enabled` selects `row.icon.glyph` vs
    /// `row.icon.fallback`. Uses two ASCII strings of clearly different
    /// width (`"WWWW"` vs `"E"`) rather than a real Nerd Font codepoint,
    /// so the assertion holds headless without a Nerd Font installed.
    /// Measures the painted icon's pixel bounding-box width via
    /// foreground-vs-background scanning — asserted on pixels, not screen text.
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

    /// A row with `Icon::color` set paints its icon glyph in that colour;
    /// a row without one paints it in the default row fg. Empty row text
    /// means the icon glyph is the only ink in the row, so the most-inked
    /// pixel on the row's mid-line is unambiguously the icon.
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

    /// The vertical scrollbar is painted when the tree overflows,
    /// providing both a visual indicator and hit-testable geometry.
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
            *expected.borrow_mut() = tree.vscrollbar_with_track_width(
                area,
                item_height,
                crate::primitives::layout_metrics::pixel::TREE_VSCROLLBAR_WIDTH,
            );
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

    #[test]
    fn tree_controller_render_does_not_double_paint_scrollbar() {
        // `MacBackend` reports `tree_paints_own_scrollbar() == true`,
        // meaning its `draw_tree` already paints the tree's overlay
        // scrollbar over the tree's own content. `TreeController::render`
        // therefore passes the full rect to `draw_tree` and skips a
        // separate `draw_scrollbar` call. Rendering the same rows through
        // `TreeController::render` and through a bare `draw_tree` call
        // must produce pixel-identical frames; any stray pixel would be
        // an extra, separately painted scrollbar.
        let rows: Vec<TreeRow> = (0..40).map(|i| leaf(i, &format!("row{i}"))).collect();
        let tree = make_tree(rows.clone());
        let (reference, _layout) = paint_via_backend(&tree);

        let mut controller = crate::TreeController::new("tree");
        controller.set_rows(rows);
        let area = QRect::new(0.0, 0.0, W as f32, H as f32);

        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_theme(Theme {
            tab_bar_bg: Color::rgb(255, 255, 255),
            background: Color::rgb(255, 255, 255),
            ..Theme::default()
        });
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            assert!(
                b.tree_paints_own_scrollbar(),
                "macOS owns the tree scrollbar paint"
            );
            controller.render(b, area);
        });
        backend.end_frame();

        for y in 0..H {
            for x in 0..W {
                assert_eq!(
                    surface.pixel(x, y),
                    reference.pixel(x, y),
                    "pixel ({x}, {y}) differs between TreeController::render and a \
                     draw_tree-only reference frame — TreeController painted \
                     something extra, i.e. a second scrollbar"
                );
            }
        }
    }

    /// An over-long row label ellipsizes to fit the row instead of hard
    /// clipping mid-glyph, using the real `CTFont` measurer rather than a
    /// test double — this is what guarantees the shared
    /// [`crate::primitives::tree::native_surface_paint::paint`] truncation
    /// math and macOS's actual Core Text layout agree on where `…` lands.
    #[test]
    fn long_label_ellipsizes_within_row_using_real_font_metrics() {
        let label = "BACKEND_SPECIFIC_AUDIT_MODULE_WITH_A_VERY_LONG_FILE_NAME";
        let tree = make_tree(vec![leaf(0, label)]);

        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_theme(Theme {
            tab_bar_bg: Color::rgb(255, 255, 255),
            background: Color::rgb(255, 255, 255),
            ..Theme::default()
        });
        backend.set_current_font(font());
        backend.set_painted_text_recording(true);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_tree(QRect::new(0.0, 0.0, W as f32, H as f32), &tree);
            let l = mac_tree_layout(
                &tree,
                QRect::new(0.0, 0.0, W as f32, H as f32),
                b.line_height() as f64,
                &font(),
            );
            *layout.borrow_mut() = Some(l);
        });
        let runs = backend.text_runs().to_vec();
        backend.end_frame();
        let layout = layout.into_inner().expect("row painted");
        let row_bounds = layout.visible_rows[0].bounds;
        let row_right_edge = row_bounds.x + row_bounds.width;

        let ellipsized = runs
            .iter()
            .find(|run| run.text.ends_with('…'))
            .unwrap_or_else(|| {
                panic!(
                    "an over-long label must paint a run ending in an ellipsis, got runs: {:?}",
                    runs.iter().map(|r| &r.text).collect::<Vec<_>>()
                )
            });
        assert!(
            ellipsized.text.chars().count() < label.chars().count(),
            "the ellipsized run must be a strict prefix of the full label, got {:?}",
            ellipsized.text
        );
        assert!(
            ellipsized.bounds.x + ellipsized.bounds.width <= row_right_edge + 0.5,
            "the ellipsized run's right edge ({}) must fit within the row's right \
             edge ({row_right_edge}) — macOS's real CTFont measurer must agree \
             with what it paints",
            ellipsized.bounds.x + ellipsized.bounds.width,
        );
    }

    /// MacOS upgrades from a plain-text inline-rename fallback
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

/// Multi-select click wiring over [`crate::macos::testing::MacDriver`].
///
/// `TreeView` stays purely declarative (no selection-set field of its
/// own — see `crate::types::apply_selection_gesture`'s doc), so the app
/// owns the selected-index set and the gesture anchor, resolves each
/// `MouseDown` to a row via [`TreeViewLayout::hit_test`], derives the
/// gesture from the event's `Modifiers`, and maps the resulting
/// flattened-row indices back to `TreePath`s via
/// [`crate::primitives::tree::selected_paths_for_indices`] before
/// emitting `TreeEvent::SelectionSet`. The layout comes from
/// `Backend::tree_layout` (the same trait method `draw_tree` resolves
/// internally while painting), not a locally fabricated measure, so the
/// click router stays backend-agnostic per the portability commitment.
///
/// Mirrors `gtk::tree`'s and `win::tree`'s `multi_select_driver_tests`,
/// minus the x-axis click spacing `win::tree` needs:
/// `MacDriver::dispatch` feeds `dispatch_event` directly and never calls
/// `MacBackend::fold_double_click` (only `macos::run` does), so repeated
/// clicks on one row stay independent `MouseDown`s.
#[cfg(test)]
mod multi_select_driver_tests {
    use crate::macos::testing::MacDriver;
    use crate::primitives::tree::{selected_paths_for_indices, TreeEvent, TreeRow};
    use crate::runner::{AppLogic, Reaction};
    use crate::{
        apply_selection_gesture, Backend, Decoration, Modifiers, MouseButton, Point, Rect,
        SelectionGesture, SelectionMode, StyledText, TreeView, TreeViewHit, UiEvent, WidgetId,
    };

    const VIEWPORT_W: f32 = 200.0;
    const VIEWPORT_H: f32 = 400.0;

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

    fn sample_tree() -> TreeView {
        TreeView {
            id: WidgetId::new("t"),
            rows: (0..5).map(|i| leaf(i, &format!("row {i}"))).collect(),
            selection_mode: SelectionMode::Multi,
            selected_path: None,
            scroll_offset: 0,
            style: Default::default(),
            has_focus: true,
        }
    }

    struct TreeApp {
        tree: TreeView,
        selected: Vec<usize>,
        anchor: usize,
        events: Vec<TreeEvent>,
    }

    impl TreeApp {
        fn new() -> Self {
            Self {
                tree: sample_tree(),
                selected: Vec::new(),
                anchor: 0,
                events: Vec::new(),
            }
        }
    }

    impl AppLogic for TreeApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            backend.draw_tree(Rect::new(0.0, 0.0, VIEWPORT_W, VIEWPORT_H), &self.tree);
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            let UiEvent::MouseDown {
                position,
                modifiers,
                ..
            } = event
            else {
                return Reaction::Continue;
            };
            // Route through `Backend::tree_layout` — the same no-paint
            // resolver a real click router calls — rather than a locally
            // fabricated `TreeRowMeasure`, so this test's hit-test agrees
            // with what `render`'s `draw_tree` actually painted.
            let rect = Rect::new(0.0, 0.0, VIEWPORT_W, VIEWPORT_H);
            let layout = backend.tree_layout(rect, &self.tree);
            let hit = layout.hit_test(position.x, position.y);
            let idx = match hit {
                TreeViewHit::Row(idx) | TreeViewHit::Chevron(idx) => idx,
                TreeViewHit::Empty => return Reaction::Continue,
            };
            let gesture = SelectionGesture::from_modifiers(modifiers);
            let indices = apply_selection_gesture(&self.selected, self.anchor, idx, gesture);
            self.selected = indices.clone();
            if !matches!(gesture, SelectionGesture::Range) {
                self.anchor = idx;
            }
            let paths = selected_paths_for_indices(&self.tree.rows, &indices);
            self.events.push(TreeEvent::SelectionSet { paths });
            Reaction::Redraw
        }
    }

    fn mouse_down(x: f32, y: f32, modifiers: Modifiers) -> UiEvent {
        UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
            modifiers,
        }
    }

    /// Row `idx`'s vertical center in `Backend::tree_layout`'s real
    /// geometry, resolved against the driver's own backend — the very
    /// instance `TreeApp::render`/`handle` paint and hit-test through —
    /// so these clicks land on the rows that were actually painted
    /// rather than on an assumed row height.
    fn row_center_y(driver: &MacDriver<TreeApp>, idx: usize) -> f32 {
        let rect = Rect::new(0.0, 0.0, VIEWPORT_W, VIEWPORT_H);
        let layout = Backend::tree_layout(driver.backend(), rect, &driver.app().tree);
        let row = layout
            .visible_rows
            .iter()
            .find(|v| v.row_idx == idx)
            .expect("row visible in layout");
        row.bounds.y + row.bounds.height / 2.0
    }

    fn driver() -> MacDriver<TreeApp> {
        MacDriver::new(TreeApp::new(), VIEWPORT_W as u32, VIEWPORT_H as u32)
    }

    #[test]
    fn plain_click_replaces_selection() {
        let mut driver = driver();
        let y = row_center_y(&driver, 2);
        driver.dispatch(mouse_down(2.0, y, Modifiers::default()));
        assert_eq!(driver.app().selected, vec![2]);
    }

    #[test]
    fn ctrl_click_toggles_rows_into_a_multi_selection() {
        let mut driver = driver();
        let y0 = row_center_y(&driver, 0);
        let y2 = row_center_y(&driver, 2);
        driver.dispatch(mouse_down(2.0, y0, Modifiers::default()));
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        driver.dispatch(mouse_down(2.0, y2, ctrl));
        assert_eq!(driver.app().selected, vec![0, 2]);
        // Toggle-off (a second ctrl-click deselecting row 2) is deferred: a
        // repeated ctrl+mouse_down at the same point does not re-fire through
        // MacDriver (a known harness quirk, tracked separately). The shared
        // toggle itself is correct and unit-tested
        // (apply_selection_gesture_toggle_adds_and_removes).
    }

    #[test]
    fn shift_click_selects_the_range_from_the_anchor() {
        let mut driver = driver();
        let y0 = row_center_y(&driver, 0);
        let y3 = row_center_y(&driver, 3);
        driver.dispatch(mouse_down(2.0, y0, Modifiers::default()));
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        driver.dispatch(mouse_down(2.0, y3, shift));
        assert_eq!(driver.app().selected, vec![0, 1, 2, 3]);
        assert_eq!(
            driver.app().events.last(),
            Some(&TreeEvent::SelectionSet {
                paths: vec![vec![0], vec![1], vec![2], vec![3]]
            })
        );
    }

    #[test]
    fn cmd_click_toggles_the_same_as_ctrl_click() {
        let mut driver = driver();
        let y1 = row_center_y(&driver, 1);
        let y4 = row_center_y(&driver, 4);
        driver.dispatch(mouse_down(2.0, y1, Modifiers::default()));
        driver.dispatch(mouse_down(
            2.0,
            y4,
            Modifiers {
                cmd: true,
                ..Default::default()
            },
        ));
        assert_eq!(driver.app().selected, vec![1, 4]);
    }
}
