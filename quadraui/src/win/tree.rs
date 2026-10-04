//! Direct2D / DirectWrite rasteriser for [`crate::TreeView`] (issue #26).
//!
//! Content painting (background, rows, chevron/icon/badge/text or
//! inline-edit, vertical scrollbar) moved to the shared
//! [`crate::primitives::tree::native_surface_paint::paint`] (#1075,
//! `PaintSurface` Phase 4 slice 2/8) — see that fn's module doc for
//! what's shared. Notably: this rasteriser previously never painted
//! `TreeRow::edit` at all (see *Scope for #26* below — inherited from
//! before this migration, kept for the historical record); it now
//! shares GTK's full caret + selection-highlight + placeholder
//! treatment like every other backend.
//!
//! [`TreeView::layout`] (the D6 layout API) does every positioning and
//! row-clipping decision; this module only estimates row geometry
//! (chevron width is an estimate, not a real DirectWrite measurement —
//! see [`win_tree_layout`]'s doc, same shortcut `gtk::tree::gtk_tree_layout`
//! takes). Paint and hit-test both derive from one [`win_tree_layout`]
//! call, so they can't drift apart.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod tree;` and `backend.rs`'s module
//! docs for why the rest of this repo's `--features win` compile gate
//! stays meaningful without a Windows host.
//!
//! Takes the live theme as a `&Theme` parameter, matching every other
//! Win-GUI rasteriser's convention — the caller
//! ([`crate::win::WinBackend::draw_tree`]) passes `&self.current_theme`,
//! the same field `Backend::set_theme` writes.
//!
//! # Scope for #26 (historical)
//!
//! Inline row editing ([`TreeRow::edit`]) was not painted prior to
//! #1075 — rows with `edit: Some(_)` rendered their normal (stale)
//! label instead. Fixed by the migration above.
//!
//! Nerd-Font icon glyphs: `WinBackend` now tracks a `nerd_fonts_enabled`
//! flag the same way `TuiBackend`/`GtkBackend`/`MacBackend` do (#804),
//! and this rasteriser picks [`crate::types::Icon::glyph`] vs
//! [`crate::types::Icon::fallback`] the same way theirs do. `draw_tree`
//! is the only Win-GUI rasteriser wired to the flag so far —
//! `win::activity_bar` still always paints `fallback` (see that
//! module's doc); extending the rest of the icon-bearing Win-GUI
//! rasterisers is separate, unstarted scope. The chosen glyph paints in
//! [`crate::types::Icon::color`] when set (#1057), else the row's
//! `def_fg`.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::DWrite;
use crate::event::Rect;
use crate::primitives::tree::{TreeView, TreeViewLayout};
use crate::theme::Theme;

/// Compute a [`TreeView`]'s layout without painting — the DirectWrite
/// twin of [`draw_tree`]'s internal layout call. `line_height` is the
/// backend's resolved line height (DIPs); row pitch and chevron
/// boundary are derived from it exactly as [`draw_tree`] does, so a
/// no-paint hit-test call always agrees with what the last paint drew.
///
/// Thin wrapper over [`crate::primitives::layout_metrics::tree_layout`]
/// (#499, adopted for `win/` by #701) — the row-pitch/chevron math is
/// identical across every pixel backend, so it lives there once instead
/// of once per backend. `chevron_end_x` is a **layout estimate**
/// (`line_height * 0.65` for the glyph width), not a real
/// `DWrite::measure_text` call — same shortcut `gtk::tree::gtk_tree_layout`
/// / `macos::tree::mac_tree_layout` take, since exact glyph metrics
/// aren't available without laying out each chevron per row.
pub fn win_tree_layout(tree: &TreeView, rect: Rect, line_height: f32) -> TreeViewLayout {
    crate::primitives::layout_metrics::tree_layout(tree, rect, line_height as f64)
}

/// Draw a [`TreeView`] into `rect` (DIPs) on `target`. Returns the
/// resolved [`TreeViewLayout`] for host click dispatch — hit regions
/// are tree-local (relative to `rect.x` / `rect.y`), matching every
/// other backend's `draw_tree` contract.
///
/// # Visual contract
///
/// - **Background:** `Theme::tab_bar_bg`.
/// - **Header rows** (`Decoration::Header`): `header_bg` / `header_fg`.
/// - **Selected row** (when `tree.has_focus`): `selected_bg` /
///   `header_fg`.
/// - **Inactive-selected row** (selected but `!tree.has_focus`):
///   `inactive_selected_bg` / `foreground`.
/// - **Muted / Error / Warning rows**: `muted_fg` / `error_fg` /
///   `warning_fg` on the row's own background.
/// - **Indent:** `(line_height * 0.9).round()` DIPs per depth level.
/// - **Chevrons:** `tree.style.chevron_expanded` /
///   `chevron_collapsed` when `tree.style.show_chevrons`; leaves get a
///   `line_height * 0.8` leading offset for alignment.
/// - **Badge** (right-aligned): `badge.fg`/`badge.bg`, falling back to
///   `muted_fg` / the row's own background.
///
/// `nerd_fonts_enabled` picks `row.icon.glyph` when `true` and
/// `row.icon.fallback` when `false` (#804) — pass `WinBackend`'s own
/// flag (set via [`crate::Backend::set_nerd_fonts`]), same convention
/// as the TUI/GTK/macOS `draw_tree` rasterisers.
pub fn draw_tree(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    tree: &TreeView,
    line_height: f32,
    nerd_fonts_enabled: bool,
    theme: &Theme,
) -> TreeViewLayout {
    let layout = win_tree_layout(tree, rect, line_height);

    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    crate::primitives::tree::native_surface_paint::paint(
        tree,
        rect,
        &layout,
        line_height,
        nerd_fonts_enabled,
        &mut surface,
        theme,
    );

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::tree::{TreeRow, TreeViewHit};
    use crate::types::{Badge, Decoration, Icon, SelectionMode, StyledText, TreeStyle, WidgetId};
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 200.0;
    const H: f32 = 200.0;
    const LINE_HEIGHT: f32 = 14.0;

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

    fn branch(idx: u16, label: &str, expanded: bool) -> TreeRow {
        TreeRow {
            path: vec![idx],
            indent: 0,
            icon: Some(Icon::new("", "D")),
            text: StyledText::plain(label.to_string()),
            badge: Some(Badge::plain("3")),
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

    /// Paint↔click round trip: painted row backgrounds and the
    /// independently-computed `win_tree_layout` must agree on which
    /// row a click lands on, including after a scroll offset.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut tree = make_tree(vec![
            branch(0, "src", true),
            leaf(1, "main.rs"),
            leaf(2, "lib.rs"),
            leaf(3, "util.rs"),
        ]);
        tree.selected_path = Some(vec![2]);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_tree(
                    target,
                    &dwrite,
                    rect,
                    &tree,
                    LINE_HEIGHT,
                    false,
                    &Theme::default(),
                );
            })
            .map(|_| win_tree_layout(&tree, rect, LINE_HEIGHT))
            .expect("paint tree");

        assert_eq!(layout.visible_rows.len(), 4, "all four rows should fit");

        for vis in &layout.visible_rows {
            let cx = vis.bounds.x + vis.bounds.width / 2.0;
            let cy = vis.bounds.y + vis.bounds.height / 2.0;
            let hit = layout.hit_test(cx, cy);
            match hit {
                TreeViewHit::Row(idx) | TreeViewHit::Chevron(idx) => {
                    assert_eq!(
                        idx, vis.row_idx,
                        "row centre should hit-test back to itself"
                    );
                }
                other => panic!("expected a row hit at row {}, got {:?}", vis.row_idx, other),
            }
        }

        // Selected row (index 2) painted `selected_bg` at its own bounds.
        let theme = Theme::default();
        let sel_bounds = layout.visible_rows[2].bounds;
        let px = surface.pixel_at((sel_bounds.x + 1.0) as u32, (sel_bounds.y + 1.0) as u32);
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
            "selected row should paint selected_bg at its own bounds"
        );
    }

    /// A click on a branch row's chevron zone resolves to `Chevron`,
    /// and to the right of it resolves to `Row` — mirrors
    /// `gtk::tree`'s chevron split tests.
    #[test]
    fn chevron_and_body_hit_split() {
        let tree = make_tree(vec![branch(0, "src", true)]);
        let rect = Rect::new(0.0, 0.0, W, H);
        let layout = win_tree_layout(&tree, rect, LINE_HEIGHT);

        let hit = layout.hit_test(1.0, layout.visible_rows[0].bounds.y + 1.0);
        assert!(matches!(hit, TreeViewHit::Chevron(0)), "got {:?}", hit);

        let hit = layout.hit_test(100.0, layout.visible_rows[0].bounds.y + 1.0);
        assert!(matches!(hit, TreeViewHit::Row(0)), "got {:?}", hit);
    }

    /// Scroll-offset round trip: after scrolling 2 rows down, the
    /// topmost visible row is `rows[2]` and a click on its painted
    /// bounds resolves to `Row(2)` — catches paint/hit-test scroll
    /// drift.
    #[test]
    fn scroll_offset_paint_and_click_agree() {
        let mut tree = make_tree((0..8).map(|i| leaf(i, &format!("file-{i}.rs"))).collect());
        tree.scroll_offset = 2;
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let layout = surface
            .paint(|target| {
                draw_tree(
                    target,
                    &dwrite,
                    rect,
                    &tree,
                    LINE_HEIGHT,
                    false,
                    &Theme::default(),
                );
            })
            .map(|_| win_tree_layout(&tree, rect, LINE_HEIGHT))
            .expect("paint");

        let first = layout.visible_rows.first().expect("has visible rows");
        assert_eq!(
            first.row_idx, 2,
            "scroll_offset=2 should put rows[2] at top"
        );
        let hit = layout.hit_test(
            first.bounds.x + 5.0,
            first.bounds.y + first.bounds.height / 2.0,
        );
        assert!(matches!(hit, TreeViewHit::Row(2)), "got {:?}", hit);
    }

    /// A click below the last row returns `Empty`.
    #[test]
    fn click_below_last_row_returns_empty() {
        let tree = make_tree(vec![leaf(0, "a"), leaf(1, "b")]);
        let rect = Rect::new(0.0, 0.0, W, H);
        let layout = win_tree_layout(&tree, rect, LINE_HEIGHT);
        let last = layout.visible_rows.last().expect("has rows");
        let hit = layout.hit_test(10.0, last.bounds.y + last.bounds.height + 5.0);
        assert!(matches!(hit, TreeViewHit::Empty), "got {:?}", hit);
    }

    /// No-paint layout must agree byte-for-byte with what `draw_tree`
    /// painted.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let tree = make_tree(vec![branch(0, "src", true), leaf(1, "main.rs")]);
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_tree(
                    target,
                    &dwrite,
                    rect,
                    &tree,
                    LINE_HEIGHT,
                    false,
                    &Theme::default(),
                );
            })
            .map(|_| win_tree_layout(&tree, rect, LINE_HEIGHT))
            .expect("paint");
        let no_paint = win_tree_layout(&tree, rect, LINE_HEIGHT);
        assert_eq!(painted, no_paint);
    }

    /// #804: `nerd_fonts_enabled` selects `row.icon.glyph` vs
    /// `row.icon.fallback` — previously `WinBackend` had no such flag at
    /// all and `draw_tree` always painted `fallback`. Uses two ASCII
    /// strings of clearly different width (`"WWWW"` vs `"E"`) rather
    /// than a real Nerd Font codepoint, so the assertion holds headless
    /// without a Nerd Font installed — same reasoning as
    /// `gtk::activity_bar::nerd_fonts_flag_selects_glyph_or_fallback` /
    /// `macos::tree::nerd_fonts_flag_selects_glyph_or_fallback`.
    /// Measures the painted icon's pixel bounding-box width via
    /// foreground-vs-background scanning — asserted on pixels, not
    /// screen text, per quadraui#555. This test was observed RED
    /// against the pre-fix `draw_tree` (both widths equal to the
    /// `"E"` fallback's).
    #[test]
    fn nerd_fonts_flag_selects_glyph_or_fallback() {
        let row = TreeRow {
            path: vec![0],
            indent: 0,
            icon: Some(Icon::new("WWWW", "E")),
            text: StyledText::plain(String::new()),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: None,
        };
        let tree = make_tree(vec![row]);
        let rect = Rect::new(0.0, 0.0, W, H);
        let theme = Theme::default();
        let bg = (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b);

        let painted_width = |nerd_fonts_enabled: bool| -> u32 {
            let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
            let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
            surface
                .paint(|target| {
                    draw_tree(
                        target,
                        &dwrite,
                        rect,
                        &tree,
                        LINE_HEIGHT,
                        nerd_fonts_enabled,
                        &Theme::default(),
                    );
                })
                .expect("paint");

            // Single row's band starts at y = 0; probe its vertical
            // mid-point.
            let mid_y = (LINE_HEIGHT * 1.4 / 2.0) as u32;
            let mut left = None;
            let mut right = None;
            for x in 0..(W as u32) {
                let px = surface.pixel_at(x, mid_y);
                if (px.r, px.g, px.b) != bg {
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
    /// row's mid-line is unambiguously the icon. This test passes
    /// `&Theme::default()` explicitly and reads its expected colours
    /// from the same instance, so it stays correct regardless of what
    /// `draw_tree`'s caller wires through in production.
    #[test]
    fn icon_color_paints_icon_glyph_in_that_color_else_default_fg() {
        let icon_color = crate::types::Color::rgb(220, 80, 20);
        let theme = Theme::default();
        let default_fg = theme.foreground;
        let bg = theme.tab_bar_bg;

        let dist = |a: (u8, u8, u8), c: crate::types::Color| {
            let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
            d(a.0, c.r) + d(a.1, c.g) + d(a.2, c.b)
        };

        let most_inked_on_row = |icon: Icon| -> (u8, u8, u8) {
            let row = TreeRow {
                path: vec![0],
                indent: 0,
                icon: Some(icon),
                text: StyledText::plain(String::new()),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit: None,
            };
            let tree = make_tree(vec![row]);
            let rect = Rect::new(0.0, 0.0, W, H);
            let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
            let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
            surface
                .paint(|target| {
                    draw_tree(
                        target,
                        &dwrite,
                        rect,
                        &tree,
                        LINE_HEIGHT,
                        false,
                        &Theme::default(),
                    );
                })
                .expect("paint");

            let mid_y = (LINE_HEIGHT * 1.4 / 2.0) as u32;
            let mut best = (bg.r, bg.g, bg.b);
            let mut best_d = -1i32;
            for x in 0..(W as u32) {
                let px = surface.pixel_at(x, mid_y);
                let c = (px.r, px.g, px.b);
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

    /// #1075 regression: before this migration, `win::tree::draw_tree`
    /// never painted `TreeRow::edit` at all — a row mid-rename rendered
    /// its stale label instead of any edit-state affordance (see this
    /// module's pre-migration doc, "Scope for #26"). This test paints a
    /// row with `edit: Some(_)` and asserts its interior contains
    /// painted (non-background) pixels distinct from the plain label it
    /// would have shown pre-fix — observed RED against the pre-#1075
    /// body (which painted `"old-name"`, not the edit state, at that
    /// same probed position once `edit.text` differs from the label).
    #[test]
    fn paints_row_being_edited() {
        // Differential design, not a bare "something is painted" check:
        // a bare presence check would also pass against the pre-#1075
        // body, which ignored `row.edit` and painted the row's *label*
        // regardless — this row's `text` is a single narrow glyph
        // ("x"), while its `edit.text` is a much wider string, so the
        // rightmost painted column tells the two apart. Pre-fix, both
        // trees below paint identically (the label "x"); post-fix, the
        // edited tree paints substantially further right.
        let rightmost_painted_x = |edit: Option<crate::primitives::tree::TreeRowEditState>| -> u32 {
            let row = TreeRow {
                path: vec![0],
                indent: 0,
                icon: None,
                text: StyledText::plain("x".to_string()),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit,
            };
            let tree = make_tree(vec![row]);
            let rect = Rect::new(0.0, 0.0, W, H);
            let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
            let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
            let layout = surface
                .paint(|target| {
                    draw_tree(
                        target,
                        &dwrite,
                        rect,
                        &tree,
                        LINE_HEIGHT,
                        false,
                        &Theme::default(),
                    );
                })
                .map(|_| win_tree_layout(&tree, rect, LINE_HEIGHT))
                .expect("paint tree");

            let theme = Theme::default();
            let bg = (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b);
            let row0 = &layout.visible_rows[0];
            let y = (row0.bounds.y + row0.bounds.height / 2.0) as u32;
            let mut rightmost = 0u32;
            for x in (row0.bounds.x as u32)..(row0.bounds.x + row0.bounds.width) as u32 {
                let px = surface.pixel_at(x.min(W as u32 - 1), y.min(H as u32 - 1));
                if (px.r, px.g, px.b) != bg {
                    rightmost = x;
                }
            }
            rightmost
        };

        let plain_label_x = rightmost_painted_x(None);
        let editing_x = rightmost_painted_x(Some(crate::primitives::tree::TreeRowEditState {
            text: "a much wider inline-rename string".into(),
            cursor: 3,
            selection_anchor: None,
            placeholder: None,
        }));

        assert!(
            editing_x > plain_label_x + 20,
            "a row with `edit: Some(_)` carrying much wider text must paint \
             noticeably further right ({editing_x}) than the same row's plain \
             label ({plain_label_x}) — this is the #1075 fix for Win never \
             painting TreeRow::edit at all (pre-fix, both values were equal)"
        );
    }

    /// #1075 regression: before this migration, `draw_tree` never
    /// painted a vertical scrollbar even though `Backend::tree_vscrollbar`
    /// (#1043) already returned real geometry for hit-testing.
    #[test]
    fn paints_vertical_scrollbar_track_when_overflowing() {
        let tree = make_tree((0..40).map(|i| leaf(i, &format!("row-{i}"))).collect());
        let rect = Rect::new(0.0, 0.0, W, H);
        let item_height =
            crate::primitives::layout_metrics::tree_row_pitch(&tree, LINE_HEIGHT as f64) as f32;
        let expected = tree
            .vscrollbar(rect, item_height)
            .expect("40 rows in this viewport must need a v-scrollbar");

        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        surface
            .paint(|target| {
                draw_tree(
                    target,
                    &dwrite,
                    rect,
                    &tree,
                    LINE_HEIGHT,
                    false,
                    &Theme::default(),
                );
            })
            .expect("paint tree");

        let theme = Theme::default();
        let probe_x = (expected.track.x + expected.track.width / 2.0) as u32;
        let probe_y = (expected.track.y + expected.track.height / 2.0) as u32;
        let px = surface.pixel_at(probe_x.min(W as u32 - 1), probe_y.min(H as u32 - 1));
        assert_ne!(
            (px.r, px.g, px.b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "expected the v-scrollbar track at ({probe_x}, {probe_y}) to be painted \
             (non-background) — the #1075 regression this test guards"
        );
    }
}
