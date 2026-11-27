//! TUI entry point for [`crate::MessageList`]: paints it through the
//! widget's own cell painter over a [`ratatui::buffer::Buffer`].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::BufferCellSurface;
use crate::primitives::message_list::MessageList;
use crate::types::Color;

/// Paints `list` into `area` over `panel_bg`; see
/// [`crate::primitives::message_list`] for the row rules.
pub fn draw_message_list(buf: &mut Buffer, area: Rect, list: &MessageList, panel_bg: Color) {
    crate::primitives::message_list::paint_cells::paint(
        &mut BufferCellSurface::new(buf),
        crate::event::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ),
        list,
        panel_bg,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::message_list::MessageRow;
    use crate::theme::Theme;
    use crate::types::{StyledSpan, WidgetId};

    fn row(text: &str, indent: f32) -> MessageRow {
        MessageRow::new(text, Color::rgb(200, 200, 200), indent)
    }

    fn make_list() -> MessageList {
        let mut styled = row("", 0.0);
        styled.spans = vec![
            StyledSpan {
                bold: true,
                ..StyledSpan::with_fg("bold ", Color::rgb(255, 0, 0))
            },
            StyledSpan::plain("plain"),
        ];
        MessageList {
            id: WidgetId::new("ml"),
            rows: vec![row("skipped", 0.0), row("first", 2.0), styled],
            scroll_top: 1,
        }
    }

    /// The widget's contract cell paint reaches the buffer through the
    /// `CellSurface` verbs alone and paints what the TUI entry point does.
    #[test]
    fn contract_cell_paint_needs_only_the_verbs() {
        use crate::primitive::{LayoutCtx, Primitive};
        let w = make_list();
        let theme = Theme::default();
        let style = crate::Style::default();
        let area = Rect::new(0, 0, 20, 4);
        let mut legacy = Buffer::empty(area);
        draw_message_list(&mut legacy, area, &w, theme.background);
        let mut buf = Buffer::empty(area);
        let rect = crate::event::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        );
        let layout = Primitive::layout(&w, &LayoutCtx::new(1.0, 1.0, false), rect);
        w.paint_cells(
            &layout,
            &mut crate::tui::cell_surface::VerbsOnly(super::BufferCellSurface::new(&mut buf)),
            &crate::PaintEnv::new(&theme, &style),
        );
        assert_eq!(buf, legacy);
    }

    #[test]
    fn rows_paint_from_scroll_top_with_their_indent_and_span_weight() {
        let area = Rect::new(0, 0, 20, 4);
        let mut buf = Buffer::empty(area);
        draw_message_list(&mut buf, area, &make_list(), Color::rgb(0, 0, 0));
        let line =
            |y: u16| -> String { (0..20).map(|x| buf[(x, y)].symbol().to_string()).collect() };
        assert_eq!(line(0).trim_end(), "  first");
        assert_eq!(line(1).trim_end(), "bold plain");
        assert!(buf[(0, 1)]
            .modifier
            .contains(ratatui::style::Modifier::BOLD));
        assert!(!buf[(5, 1)]
            .modifier
            .contains(ratatui::style::Modifier::BOLD));
    }
}
