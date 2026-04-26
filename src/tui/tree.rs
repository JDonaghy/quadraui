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
use crate::primitives::tree::{TreeRowMeasure, TreeView};
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

    let layout = tree.layout(area.width as f32, area.height as f32, |_| {
        TreeRowMeasure::new(1.0)
    });

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
}
