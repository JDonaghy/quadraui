//! Backend-agnostic demo for the Markdown → [`StyledText`] adapter
//! ([`tui_markdown`] / `gtk_markdown`).
//!
//! [`MarkdownDemo`] renders a fixed markdown document through
//! [`render_markdown_to_styled`] and paints the result inside a
//! [`RichTextPopup`]. The same `AppLogic` impl drives every backend; the
//! per-backend runner files are ~3 lines each.
//!
//! The document deliberately includes `snake_case` identifiers and a
//! whitespace-flanked `*` so the flanking guard is exercised visually:
//! `foo_bar` and `a * b` must render upright, while `*italic*`, `_also_`,
//! `**bold**`, and `` `code` `` get their styling.

use quadraui::{
    render_markdown_to_styled, AppLogic, Backend, Key, NamedKey, PopupPlacement, Reaction, Rect,
    RichTextPopup, RichTextPopupMeasure, Theme, UiEvent, WidgetId,
};

/// The markdown source rendered by the demo.
const DOC: &str = "\
# Markdown adapter demo
## Headings scale up
### And down again
Body text with **bold**, *italic*, _also italic_, and `inline_code`.
Identifiers like foo_bar and baz_qux stay upright (no intraword emphasis).
Arithmetic like a * b * c is left alone too.
Use `render_markdown_to_styled` to convert this into styled lines.";

pub struct MarkdownDemo {
    scroll_top: usize,
}

impl MarkdownDemo {
    pub fn new() -> Self {
        Self { scroll_top: 0 }
    }
}

impl Default for MarkdownDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for MarkdownDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let theme = Theme::default();
        let rendered = render_markdown_to_styled(DOC, &theme);

        let viewport = backend.viewport();
        let col_w = backend.char_width();
        let row_h = backend.line_height();

        // Content width = widest line, in backend units.
        let widest = rendered
            .line_text
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0);
        let content_w = (widest as f32 + 1.0) * col_w;

        let popup = RichTextPopup {
            id: WidgetId::new("markdown:demo"),
            lines: rendered.lines,
            line_text: rendered.line_text,
            line_scales: rendered.line_scales,
            scroll_top: self.scroll_top,
            max_visible_rows: 12,
            has_focus: true,
            selection: None,
            links: Vec::new(),
            focused_link: None,
            placement: PopupPlacement::Below,
            padding: 1.0,
            fg: None,
            bg: None,
        };

        // Anchor near the top-left so "Below" placement keeps it on-screen.
        // The viewport origin is (0, 0) in every backend.
        let anchor_x = viewport.width * 0.05;
        let anchor_y = row_h;
        let measure = RichTextPopupMeasure::new(content_w, row_h);
        let vp = Rect::new(0.0, 0.0, viewport.width, viewport.height);
        let layout = popup.layout(anchor_x, anchor_y, vp, measure, |_, start, end| {
            (end - start) as f32 * col_w
        });
        backend.draw_rich_text_popup(&popup, &layout);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed { key, .. } => match key {
                Key::Char('q') | Key::Named(NamedKey::Escape) => Reaction::Exit,
                Key::Named(NamedKey::Down) => {
                    self.scroll_top = self.scroll_top.saturating_add(1);
                    Reaction::Redraw
                }
                Key::Named(NamedKey::Up) => {
                    self.scroll_top = self.scroll_top.saturating_sub(1);
                    Reaction::Redraw
                }
                _ => Reaction::Continue,
            },
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
