//! TUI rasteriser for [`crate::TreeView`].
//!
//! Per D6: this function asks the primitive for a [`crate::TreeViewLayout`]
//! using a uniform 1-cell-per-row measurer (TUI rows are always 1 cell
//! tall) and paints the resolved positions verbatim. The GTK rasteriser
//! supplies a different per-row measurer (header rows 1× line_height,
//! leaves 1.4×) — that's a backend-specific decision the rasteriser
//! owns, not the primitive.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{draw_styled_text, qc, ratatui_color, set_cell};
use crate::primitives::tree::{TreeRowMeasure, TreeView, TreeViewLayout};
use crate::theme::Theme;
use crate::types::Decoration;

/// Draw a [`TreeView`] into `area` on `buf`.
///
/// # Visual contract
///
/// - **Header rows** (`Decoration::Header`): rendered with
///   [`Theme::header_bg`] / [`Theme::header_fg`] (the SC panel's
///   section titles).
/// - **Selected row** (when `tree.has_focus`): rendered with
///   [`Theme::selected_bg`] and the row's `default_fg`.
/// - **Other rows**: rendered with [`Theme::tab_bar_bg`] /
///   [`Theme::foreground`]. Branches and leaves get the same row
///   styling — `is_expanded`-ness only affects chevron rendering, not
///   visual emphasis.
/// - **Indent:** `tree.style.indent` cells per depth level.
/// - **Chevrons:** [`tree.style.chevron_expanded`] /
///   [`tree.style.chevron_collapsed`] for branches when
///   `tree.style.show_chevrons` is true; leaves get a 2-cell leading
///   gap for visual alignment.
/// - **Icon:** `row.icon.glyph` when `nerd_fonts_enabled`, else the
///   ASCII fallback.
/// - **Badge** (right-aligned within row): rendered in
///   `badge.fg`/`badge.bg` (falling back to [`Theme::muted_fg`] /
///   row bg) when there's room past the text.
///
/// Doesn't draw a scrollbar — scrollbars are a later primitive stage.
pub fn draw_tree(
    buf: &mut Buffer,
    area: Rect,
    tree: &TreeView,
    theme: &Theme,
    nerd_fonts_enabled: bool,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let row_bg = ratatui_color(theme.tab_bar_bg);
    let hdr_bg = ratatui_color(theme.header_bg);
    let hdr_fg = ratatui_color(theme.header_fg);
    let item_fg = ratatui_color(theme.foreground);
    let sel_bg = ratatui_color(theme.selected_bg);
    let dim_fg = ratatui_color(theme.muted_fg);

    let indent_cells = tree.style.indent as usize;

    let layout = tui_tree_layout(tree, area);

    for visible_row in &layout.visible_rows {
        let row = &tree.rows[visible_row.row_idx];
        let y = area.y + visible_row.bounds.y.round() as u16;
        if y >= area.y + area.height {
            break;
        }

        let is_header = matches!(row.decoration, Decoration::Header);
        let is_selected =
            tree.has_focus && tree.selected_path.as_ref().is_some_and(|p| p == &row.path);

        let (default_fg, bg) = match (is_header, is_selected) {
            (_, true) => (hdr_fg, sel_bg),
            (true, false) => (hdr_fg, hdr_bg),
            (false, false) => (item_fg, row_bg),
        };

        for x in area.x..area.x + area.width {
            set_cell(buf, x, y, ' ', default_fg, bg);
        }

        let mut col: usize = 0;
        let indent_spaces = (row.indent as usize) * indent_cells;
        col += indent_spaces;

        if let Some(expanded) = row.is_expanded {
            if tree.style.show_chevrons {
                let chevron = if expanded {
                    &tree.style.chevron_expanded
                } else {
                    &tree.style.chevron_collapsed
                };
                for ch in chevron.chars() {
                    if col >= area.width as usize {
                        break;
                    }
                    set_cell(buf, area.x + col as u16, y, ch, default_fg, bg);
                    col += 1;
                }
                if col < area.width as usize {
                    set_cell(buf, area.x + col as u16, y, ' ', default_fg, bg);
                    col += 1;
                }
            }
        } else {
            // Leaves: small leading gap for readability.
            col += 2.min(area.width as usize - col.min(area.width as usize));
        }

        if let Some(ref icon) = row.icon {
            let glyph = if nerd_fonts_enabled {
                &icon.glyph
            } else {
                &icon.fallback
            };
            for ch in glyph.chars() {
                if col >= area.width as usize {
                    break;
                }
                set_cell(buf, area.x + col as u16, y, ch, default_fg, bg);
                col += 1;
            }
            if col < area.width as usize {
                set_cell(buf, area.x + col as u16, y, ' ', default_fg, bg);
                col += 1;
            }
        }

        let text_start = col;
        let text_end = draw_styled_text(
            buf,
            area,
            y,
            col,
            &row.text,
            default_fg,
            bg,
            row.decoration,
            dim_fg,
        );
        col = text_end;

        if let Some(ref badge) = row.badge {
            let badge_width: usize = badge.text.chars().count();
            let badge_start_col = (area.width as usize).saturating_sub(badge_width);
            if badge_start_col > text_start {
                let badge_fg = badge.fg.map(qc).unwrap_or(dim_fg);
                let badge_bg = badge.bg.map(qc).unwrap_or(bg);
                let mut bx = badge_start_col;
                for ch in badge.text.chars() {
                    if bx >= area.width as usize {
                        break;
                    }
                    set_cell(buf, area.x + bx as u16, y, ch, badge_fg, badge_bg);
                    bx += 1;
                }
            }
        }

        let _ = col;
    }
}

/// Compute the layout the TUI rasteriser would produce for `tree` in
/// `area`. Hosts and tests call this to drive hit-testing without
/// re-deriving metrics that could drift from paint. `draw_tree` uses
/// this same helper internally so paint and hit_test consume one
/// layout instance produced by one set of metrics — the source-of-
/// truth contract.
///
/// TUI rows are uniform 1-cell tall. The layout is in tree-local coords
/// (origin at 0,0); convert absolute click coords by subtracting
/// `area.x` / `area.y` before calling [`TreeViewLayout::hit_test`].
pub fn tui_tree_layout(tree: &TreeView, area: Rect) -> TreeViewLayout {
    tree.layout(area.width as f32, area.height as f32, |_| {
        TreeRowMeasure::new(1.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::tree::{TreeRow, TreeView};
    use crate::types::{StyledSpan, StyledText, TreeStyle, WidgetId};

    fn row(path: &[u16], indent: u16, label: &str, expanded: Option<bool>) -> TreeRow {
        TreeRow {
            path: path.to_vec(),
            indent,
            text: StyledText {
                spans: vec![StyledSpan::plain(label)],
            },
            icon: None,
            badge: None,
            is_expanded: expanded,
            decoration: Decoration::Normal,
        }
    }

    fn make_tree() -> TreeView {
        TreeView {
            id: WidgetId::new("tree"),
            rows: vec![
                row(&[0], 0, "src", Some(true)),
                row(&[0, 0], 1, "main.rs", None),
                row(&[0, 1], 1, "lib.rs", None),
            ],
            selection_mode: crate::types::SelectionMode::default(),
            selected_path: Some(vec![0, 0]),
            scroll_offset: 0,
            has_focus: true,
            style: TreeStyle::default(),
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn paints_branch_with_chevron_and_leaves_indented() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        let tree = make_tree();
        draw_tree(
            &mut buf,
            Rect::new(0, 0, 30, 5),
            &tree,
            &Theme::default(),
            false,
        );

        // Row 0 ("src"): branch, expanded — chevron at col 0.
        // TreeStyle::default() chevron_expanded is '▾'.
        assert_eq!(
            cell_char(&buf, 0, 0),
            tree.style.chevron_expanded.chars().next().unwrap()
        );
        // "src" should appear after the chevron + space.
        let src_row: String = (2..5).map(|x| cell_char(&buf, x, 0)).collect();
        assert_eq!(src_row, "src");

        // Row 1 ("main.rs"): leaf, indent=1 → starts at col 2 (indent_cells default).
        // Leaves get a 2-cell leading gap on top of indent.
        // Default TreeStyle::indent is 2.
        let leaf_label_start = 2 + 2;
        let main: String = (leaf_label_start..(leaf_label_start + 7))
            .map(|x| cell_char(&buf, x as u16, 1))
            .collect();
        assert_eq!(main, "main.rs");
    }

    #[test]
    fn selected_row_uses_selected_bg() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        let tree = make_tree();
        let theme = Theme {
            selected_bg: crate::types::Color::rgb(99, 0, 0),
            ..Theme::default()
        };
        draw_tree(&mut buf, Rect::new(0, 0, 30, 5), &tree, &theme, false);
        // Row 1 ("main.rs") is selected — its bg should be (99, 0, 0).
        let bg = buf[(0u16, 1u16)].bg;
        assert_eq!(bg, ratatui::style::Color::Rgb(99, 0, 0));
    }

    #[test]
    fn header_row_uses_header_bg_when_unselected() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 3));
        let tree = TreeView {
            id: WidgetId::new("tree"),
            rows: vec![TreeRow {
                path: vec![0],
                indent: 0,
                text: StyledText {
                    spans: vec![StyledSpan::plain("CHANGES")],
                },
                icon: None,
                badge: None,
                is_expanded: Some(true),
                decoration: Decoration::Header,
            }],
            selection_mode: crate::types::SelectionMode::default(),
            selected_path: None,
            scroll_offset: 0,
            has_focus: false,
            style: TreeStyle::default(),
        };
        let theme = Theme {
            header_bg: crate::types::Color::rgb(7, 7, 7),
            ..Theme::default()
        };
        draw_tree(&mut buf, Rect::new(0, 0, 30, 3), &tree, &theme, false);
        let bg = buf[(0u16, 0u16)].bg;
        assert_eq!(bg, ratatui::style::Color::Rgb(7, 7, 7));
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let tree = make_tree();
        draw_tree(
            &mut buf,
            Rect::new(0, 0, 0, 5),
            &tree,
            &Theme::default(),
            false,
        );
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    // ── Paint↔click round-trip harness ─────────────────────────────────────
    //
    // Per the Session 346 course correction (PLAN.md "🧭 Course
    // correction"): primitives that promise paint/click consistency
    // need empirical verification, not just structural design. These
    // tests paint a `TreeView` into a `ratatui::Buffer`, find the cells
    // the rasteriser actually wrote glyphs into, then hit_test those
    // exact coordinates against the layout the rasteriser used and
    // assert the hit identifies the painted-into row. If paint and
    // click ever drift in the TUI tree rasteriser, one of these fails.
    //
    // TreeView uses tree-local coords (origin at 0,0) for `hit_test`,
    // unlike `MultiSectionView` which uses absolute coords. The harness
    // converts between them by subtracting `area.x`/`area.y` from the
    // painted absolute y. Same source-of-truth contract; different
    // origin convention.
    //
    // Mirrors the MultiSectionView harness shape (#298) so future
    // primitives can copy this pattern.

    use crate::primitives::tree::TreeViewHit;
    use crate::tui::tree::tui_tree_layout;
    use crate::types::TreePath;

    fn row_text(buf: &Buffer, y: u16) -> String {
        let area = buf.area;
        (area.x..area.x + area.width)
            .map(|x| cell_char(buf, x, y))
            .collect()
    }

    fn find_row_with(buf: &Buffer, needle: &str) -> Option<u16> {
        let area = buf.area;
        for y in area.y..area.y + area.height {
            if row_text(buf, y).contains(needle) {
                return Some(y);
            }
        }
        None
    }

    fn long_tree(n: usize) -> TreeView {
        let labels: Vec<String> = (0..n).map(|i| format!("item-{i:02}")).collect();
        let rows: Vec<TreeRow> = labels
            .iter()
            .enumerate()
            .map(|(i, label)| TreeRow {
                path: vec![i as u16] as TreePath,
                indent: 0,
                text: StyledText {
                    spans: vec![StyledSpan::plain(label.clone())],
                },
                icon: None,
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
            })
            .collect();
        TreeView {
            id: WidgetId::new("long"),
            rows,
            selection_mode: crate::types::SelectionMode::default(),
            selected_path: None,
            scroll_offset: 0,
            has_focus: false,
            style: TreeStyle::default(),
        }
    }

    fn paint_then_layout(
        buf: &mut Buffer,
        area: Rect,
        tree: &TreeView,
        theme: &Theme,
        nerd_fonts_enabled: bool,
    ) -> TreeViewLayout {
        draw_tree(buf, area, tree, theme, nerd_fonts_enabled);
        tui_tree_layout(tree, area)
    }

    /// Round-trip: paint, find each row's label in the buffer, hit_test
    /// the same coordinate, assert the hit returns Row(idx) for the
    /// matching tree.rows index.
    #[test]
    fn clicks_land_on_painted_row() {
        let area = Rect::new(0, 0, 30, 8);
        let mut buf = Buffer::empty(area);
        let tree = long_tree(5);
        let layout = paint_then_layout(&mut buf, area, &tree, &Theme::default(), false);

        for visible in &layout.visible_rows {
            let needle = tree.rows[visible.row_idx]
                .text
                .spans
                .first()
                .map(|s| s.text.clone())
                .unwrap_or_default();
            let painted_y = find_row_with(&buf, &needle)
                .unwrap_or_else(|| panic!("row {} ({:?}) not painted", visible.row_idx, needle));
            // TreeView hit_test takes tree-local coords (origin at 0,0).
            let local_y = (painted_y - area.y) as f32;
            let hit = layout.hit_test(0.5, local_y);
            match hit {
                TreeViewHit::Row(idx) => assert_eq!(
                    idx, visible.row_idx,
                    "row {} ({:?}) painted at y={} but hit_test returned Row({})",
                    visible.row_idx, needle, painted_y, idx
                ),
                other => panic!(
                    "row {} ({:?}) painted at y={} but hit_test returned {:?}",
                    visible.row_idx, needle, painted_y, other
                ),
            }
        }
    }

    /// Click below the last visible row returns Empty.
    #[test]
    fn click_below_last_row_returns_empty() {
        let area = Rect::new(0, 0, 30, 8);
        let mut buf = Buffer::empty(area);
        // 2 rows in an 8-row viewport leaves 6 rows of empty space.
        let tree = long_tree(2);
        let layout = paint_then_layout(&mut buf, area, &tree, &Theme::default(), false);
        let hit = layout.hit_test(0.5, 5.0); // y=5, well below the 2 painted rows
        assert!(
            matches!(hit, TreeViewHit::Empty),
            "click at y=5 below last row returned {:?}",
            hit
        );
    }

    /// With `scroll_offset > 0`, painted row 0 must be `tree.rows[scroll_offset]`,
    /// and clicking it must return `Row(scroll_offset)` — NOT `Row(0)`.
    /// This is the bug class where paint advances by scroll_offset but
    /// click forgets, or vice versa.
    #[test]
    fn scroll_offset_paint_and_click_agree() {
        let area = Rect::new(0, 0, 30, 4);
        let mut buf = Buffer::empty(area);
        let mut tree = long_tree(10);
        tree.scroll_offset = 3;
        let layout = paint_then_layout(&mut buf, area, &tree, &Theme::default(), false);

        // Painted row 0 should be `tree.rows[3]` ("item-03").
        let painted_y = find_row_with(&buf, "item-03")
            .expect("scroll_offset=3 should make row 0 paint as 'item-03'");
        let local_y = (painted_y - area.y) as f32;
        let hit = layout.hit_test(0.5, local_y);
        assert!(
            matches!(hit, TreeViewHit::Row(3)),
            "scroll_offset=3: row painted as 'item-03' at y={}; expected Row(3), got {:?}",
            painted_y,
            hit
        );

        // Painted row 0 should NOT be the original "item-00" — that one
        // is scrolled out and shouldn't appear in the buffer at all.
        assert!(
            find_row_with(&buf, "item-00").is_none(),
            "item-00 should be scrolled out under scroll_offset=3 but appears in buffer"
        );
    }

    /// Header rows (Decoration::Header) and leaf rows are equally
    /// hit-testable. Mixed-row trees should round-trip cleanly across
    /// every visible row regardless of decoration.
    #[test]
    fn header_and_leaf_rows_both_hit_testable() {
        let area = Rect::new(0, 0, 30, 6);
        let mut buf = Buffer::empty(area);
        let tree = TreeView {
            id: WidgetId::new("mixed"),
            rows: vec![
                TreeRow {
                    path: vec![0],
                    indent: 0,
                    text: StyledText {
                        spans: vec![StyledSpan::plain("CHANGES")],
                    },
                    icon: None,
                    badge: None,
                    is_expanded: Some(true),
                    decoration: Decoration::Header,
                },
                row(&[0, 0], 1, "alpha", None),
                row(&[0, 1], 1, "beta", None),
            ],
            selection_mode: crate::types::SelectionMode::default(),
            selected_path: None,
            scroll_offset: 0,
            has_focus: false,
            style: TreeStyle::default(),
        };
        let layout = paint_then_layout(&mut buf, area, &tree, &Theme::default(), false);

        for (expected_idx, needle) in [(0usize, "CHANGES"), (1, "alpha"), (2, "beta")] {
            let painted_y =
                find_row_with(&buf, needle).unwrap_or_else(|| panic!("row {needle:?} not painted"));
            let local_y = (painted_y - area.y) as f32;
            let hit = layout.hit_test(0.5, local_y);
            assert!(
                matches!(hit, TreeViewHit::Row(idx) if idx == expected_idx),
                "row {needle:?} painted at y={painted_y}: expected Row({expected_idx}), got {hit:?}"
            );
        }
    }
}
