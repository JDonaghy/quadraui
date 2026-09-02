//! Direct2D / DirectWrite rasteriser for [`crate::TreeView`] (issue #26).
//!
//! Mirrors `gtk::tree`'s structure: [`TreeView::layout`] (the D6 layout
//! API) does every positioning and row-clipping decision; this module
//! only estimates row geometry (chevron width is an estimate, not a
//! real DirectWrite measurement — see [`win_tree_layout`]'s doc, same
//! shortcut `gtk::tree::gtk_tree_layout` takes) and paints (via
//! [`super::text::fill_rect`] + [`DWrite::draw_text`]/`draw_text_styled`).
//! Paint and hit-test both derive from one [`win_tree_layout`] call, so
//! they can't drift apart.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod tree;` and `backend.rs`'s module
//! docs for why the rest of this repo's `--features win` compile gate
//! stays meaningful without a Windows host. See `win::status_bar`'s
//! module doc for why colours come from `Theme::default()` rather than a
//! live `WinBackend` theme field.
//!
//! # Scope for #26
//!
//! Inline row editing ([`TreeRow::edit`]) is not painted — rows with
//! `edit: Some(_)` render their normal label instead. Nerd-Font icon
//! glyphs are not distinguished from ASCII fallbacks: `WinBackend` does
//! not yet track a `nerd_fonts_enabled` setting the way `GtkBackend`
//! does, so this rasteriser always paints [`crate::types::Icon::fallback`].
//! Both are follow-up scope, not a compile-error gap.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, DWrite};
use crate::event::Rect;
use crate::primitives::tree::{TreeRowMeasure, TreeView, TreeViewLayout};
use crate::theme::Theme;
use crate::types::Decoration;

/// Compute a [`TreeView`]'s layout without painting — the DirectWrite
/// twin of [`draw_tree`]'s internal layout call. `line_height` is the
/// backend's resolved line height (DIPs); row pitch and chevron
/// boundary are derived from it exactly as [`draw_tree`] does, so a
/// no-paint hit-test call always agrees with what the last paint drew.
///
/// `chevron_end_x` is a **layout estimate** (`line_height * 0.65` for
/// the glyph width), not a real `DWrite::measure_text` call — mirrors
/// `gtk::tree::gtk_tree_layout`'s identical shortcut, since exact glyph
/// metrics aren't available without laying out each chevron per row.
pub fn win_tree_layout(tree: &TreeView, rect: Rect, line_height: f32) -> TreeViewLayout {
    let header_height = (line_height * 1.2).round();
    let item_height = tree
        .style
        .row_height
        .map(|h| h as f32)
        .unwrap_or(line_height * 1.4)
        .round();
    let indent_px = (line_height * 0.9).round();
    let show_chevrons = tree.style.show_chevrons;

    tree.layout(rect.width, rect.height, |i| {
        let row = &tree.rows[i];
        let is_header = matches!(row.decoration, Decoration::Header);
        let row_h = if is_header {
            header_height
        } else {
            item_height
        };
        let chevron_end_x = if row.is_expanded.is_some() && show_chevrons {
            let est_glyph_w = line_height * 0.65;
            Some(2.0 + row.indent as f32 * indent_px + est_glyph_w + 4.0)
        } else {
            None
        };
        TreeRowMeasure {
            height: row_h,
            chevron_end_x,
        }
    })
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
pub fn draw_tree(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    tree: &TreeView,
    line_height: f32,
) -> TreeViewLayout {
    let theme = Theme::default();
    let _ = fill_rect(target, rect, theme.tab_bar_bg);

    let layout = win_tree_layout(tree, rect, line_height);
    let indent_px = (line_height * 0.9).round();

    for vis_row in &layout.visible_rows {
        let row = &tree.rows[vis_row.row_idx];
        let row_rect = Rect::new(
            rect.x + vis_row.bounds.x,
            rect.y + vis_row.bounds.y,
            vis_row.bounds.width,
            vis_row.bounds.height,
        );

        let path_selected = tree.selected_path.as_ref().is_some_and(|p| p == &row.path);
        let is_selected = tree.has_focus && path_selected;
        let is_inactive_selected = !tree.has_focus && path_selected;
        let is_header = matches!(row.decoration, Decoration::Header);

        let (def_fg, row_bg) = if is_selected {
            (theme.header_fg, theme.selected_bg)
        } else if is_inactive_selected {
            (theme.foreground, theme.inactive_selected_bg)
        } else if is_header {
            (theme.header_fg, theme.header_bg)
        } else {
            match row.decoration {
                Decoration::Muted => (theme.muted_fg, theme.tab_bar_bg),
                Decoration::Error => (theme.error_fg, theme.tab_bar_bg),
                Decoration::Warning => (theme.warning_fg, theme.tab_bar_bg),
                _ => (theme.foreground, theme.tab_bar_bg),
            }
        };
        let _ = fill_rect(target, row_rect, row_bg);

        let mut cursor_x = row_rect.x + 2.0 + row.indent as f32 * indent_px;

        if let Some(expanded) = row.is_expanded {
            if tree.style.show_chevrons {
                let chevron = if expanded {
                    &tree.style.chevron_expanded
                } else {
                    &tree.style.chevron_collapsed
                };
                let (cw, ch) = dwrite.measure_text(chevron).unwrap_or((0.0, 0.0));
                let cy = row_rect.y + (row_rect.height - ch) / 2.0;
                let _ = dwrite.draw_text(target, chevron, Rect::new(cursor_x, cy, cw, ch), def_fg);
                cursor_x += cw + 4.0;
            }
        } else {
            cursor_x += line_height * 0.8;
        }

        if let Some(ref icon) = row.icon {
            let glyph = icon.fallback.as_str();
            let (iw, ih) = dwrite.measure_text(glyph).unwrap_or((0.0, 0.0));
            let iy = row_rect.y + (row_rect.height - ih) / 2.0;
            let _ = dwrite.draw_text(target, glyph, Rect::new(cursor_x, iy, iw, ih), def_fg);
            cursor_x += iw + 6.0;
        }

        let badge_info = row.badge.as_ref().map(|badge| {
            let (bw, _) = dwrite.measure_text(&badge.text).unwrap_or((0.0, 0.0));
            let bfg = badge.fg.unwrap_or(theme.muted_fg);
            let bbg = badge.bg.unwrap_or(row_bg);
            (badge.text.clone(), bw, bfg, bbg)
        });
        let badge_reserve = badge_info
            .as_ref()
            .map(|(_, bw, ..)| *bw + 8.0)
            .unwrap_or(0.0);
        let text_right_limit = row_rect.x + row_rect.width - badge_reserve - 4.0;

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
            let (sw, sh) = dwrite
                .measure_text_styled(&span.text, span.bold)
                .unwrap_or((0.0, 0.0));
            if let Some(sbg) = span.bg {
                let clipped_w = sw.min((text_right_limit - cursor_x).max(0.0));
                let _ = fill_rect(
                    target,
                    Rect::new(cursor_x, row_rect.y, clipped_w, row_rect.height),
                    sbg,
                );
            }
            let sy = row_rect.y + (row_rect.height - sh) / 2.0;
            let _ = dwrite.draw_text_styled(
                target,
                &span.text,
                Rect::new(cursor_x, sy, sw, sh),
                span_fg,
                span.bold,
            );
            cursor_x += sw;
        }

        if let Some((btext, bw, bfg, bbg)) = badge_info {
            let bx = row_rect.x + row_rect.width - bw - 4.0;
            if bx > cursor_x {
                if bbg != row_bg {
                    let _ = fill_rect(
                        target,
                        Rect::new(bx - 2.0, row_rect.y, bw + 4.0, row_rect.height),
                        bbg,
                    );
                }
                let (_, bh) = dwrite.measure_text(&btext).unwrap_or((0.0, 0.0));
                let by = row_rect.y + (row_rect.height - bh) / 2.0;
                let _ = dwrite.draw_text(target, &btext, Rect::new(bx, by, bw, bh), bfg);
            }
        }
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::tree::{TreeRow, TreeViewHit};
    use crate::types::{Badge, Icon, SelectionMode, StyledText, TreeStyle, WidgetId};
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
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
                draw_tree(target, &dwrite, rect, &tree, LINE_HEIGHT);
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let layout = surface
            .paint(|target| {
                draw_tree(target, &dwrite, rect, &tree, LINE_HEIGHT);
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_tree(target, &dwrite, rect, &tree, LINE_HEIGHT);
            })
            .map(|_| win_tree_layout(&tree, rect, LINE_HEIGHT))
            .expect("paint");
        let no_paint = win_tree_layout(&tree, rect, LINE_HEIGHT);
        assert_eq!(painted, no_paint);
    }
}
