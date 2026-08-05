//! Demo for [`render_markdown_to_styled_wrapped`]: word-wrapped markdown rows
//! displayed in a [`ListView`].
//!
//! This is the canonical consumer pattern for a host's scrollable
//! markdown-body panels: call [`render_markdown_to_styled_wrapped`] with
//! the current content width, convert each output row to a [`ListItem`],
//! and hand the list to the backend. Long paragraphs reflow to the
//! viewport; code blocks are intentionally never wrapped.
//!
//! Keys:
//! - `j` / `↓`  — scroll down
//! - `k` / `↑`  — scroll up
//! - `q` / `Esc` — quit

use quadraui::{
    render_markdown_to_styled_wrapped, AppLogic, Backend, Decoration, Key, ListItem, ListView,
    NamedKey, Reaction, StyledText, UiEvent, WidgetId,
};

/// Markdown source used by the demo.  It deliberately contains:
/// - A long paragraph that must wrap at any normal terminal width.
/// - Bold/italic/inline-code spans that straddle likely wrap points.
/// - A fenced code block that must **not** be wrapped.
/// - A bullet list with a long item.
const DOC: &str = "\
# Word-wrap demo

## Long paragraph (wraps)

This paragraph is intentionally verbose so that it will wrap at any \
normal terminal width. It contains **bold text that crosses the wrap \
boundary**, *italic phrases*, and even `inline_code` tokens, all of \
which must preserve their styling on every continuation row.

## Code block (never wrapped)

The following code block must remain verbatim no matter how narrow \
the viewport is:

```rust
fn render_markdown_to_styled_wrapped(input: &str, theme: &Theme, width: usize) -> RenderedMarkdown {
    // Hard split protects lines that have no word boundaries.
    todo!()
}
```

## Bullet list

- A short item.
- A much longer bullet item that should also wrap gracefully when the \
  terminal is narrow, with styling like **bold** still preserved.
- Another short one.

## Blockquote

> This is a blockquote with enough words that it will wrap at a small \
  width, and its leading pipe glyph must survive onto every visual row.
";

pub struct MarkdownWrapDemo {
    scroll_offset: usize,
    selected_idx: usize,
}

impl MarkdownWrapDemo {
    pub fn new() -> Self {
        Self {
            scroll_offset: 0,
            selected_idx: 0,
        }
    }

    /// Compute the content width available inside the list (full viewport
    /// width minus the 2-char selection prefix the rasteriser prepends).
    fn content_cols(backend: &dyn Backend) -> usize {
        let vp = backend.viewport();
        let cw = backend.char_width();
        let total_cols = (vp.width / cw).floor() as usize;
        total_cols.saturating_sub(2)
    }
}

impl Default for MarkdownWrapDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for MarkdownWrapDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let theme = quadraui::Theme::default();
        let width = Self::content_cols(backend);

        // Wrap the markdown document to the current content width.
        let rendered = render_markdown_to_styled_wrapped(DOC, &theme, width.max(10));

        // Convert each visual row to a ListItem.
        let items: Vec<ListItem> = rendered
            .lines
            .into_iter()
            .map(|styled| ListItem {
                text: styled,
                detail: None,
                icon: None,
                decoration: Decoration::Normal,
            })
            .collect();

        let n = items.len();
        let list = ListView {
            id: WidgetId::new("markdown:wrap:demo"),
            title: Some(StyledText::plain(
                " Markdown word-wrap demo (j/k scroll, q quit) ",
            )),
            items,
            selected_idx: self.selected_idx.min(n.saturating_sub(1)),
            scroll_offset: self.scroll_offset,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: true,
        };

        let vp = backend.viewport();
        backend.draw_list(quadraui::Rect::new(0.0, 0.0, vp.width, vp.height), &list);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed { key, .. } => match key {
                Key::Char('q') | Key::Named(NamedKey::Escape) => Reaction::Exit,
                Key::Char('j') | Key::Named(NamedKey::Down) => {
                    self.scroll_offset = self.scroll_offset.saturating_add(1);
                    self.selected_idx = self.selected_idx.saturating_add(1);
                    Reaction::Redraw
                }
                Key::Char('k') | Key::Named(NamedKey::Up) => {
                    self.scroll_offset = self.scroll_offset.saturating_sub(1);
                    self.selected_idx = self.selected_idx.saturating_sub(1);
                    Reaction::Redraw
                }
                _ => Reaction::Continue,
            },
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
