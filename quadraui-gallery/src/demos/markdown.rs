//! `Markdown` demo — adapted from `quadraui/examples/common/markdown_demo.rs`
//! and `markdown_wrap_demo.rs`.
//!
//! Both source files exercise the same widget — the markdown-to-
//! [`StyledText`] adapter ([`render_markdown_to_styled`] /
//! [`render_markdown_to_styled_wrapped`]) — rendered through two
//! different host primitives: a floating [`RichTextPopup`] (unwrapped,
//! horizontally scrollable) and a plain [`ListView`] of word-wrapped
//! rows. The two variants below keep both call sites, since a consumer
//! picks between them based on whether the host wants a popup overlay
//! or an inline scrollable list.

use quadraui::{
    render_markdown_to_styled, render_markdown_to_styled_wrapped, Backend, BackendCaps, Decoration,
    FontRole, Key, ListItem, ListView, NamedKey, PopupPlacement, Reaction, Rect, RichTextPopup,
    RichTextPopupMeasure, StyledText, Theme, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("markdown.rs");

// gallery:begin
/// Markdown source shared by both variants — deliberately includes
/// `snake_case` identifiers and a whitespace-flanked `*` so the
/// flanking guard is exercised visually: `foo_bar` and `a * b` render
/// upright, while `*italic*`, `_also_`, `**bold**` and `` `code` `` get
/// their styling.
const DOC: &str = "\
# Markdown adapter demo
## Headings scale up
Body text with **bold**, *italic*, _also italic_, and `inline_code`.
Identifiers like foo_bar and baz_qux stay upright (no intraword emphasis).
Arithmetic like a * b * c is left alone too.

- A short bullet item.
- A longer bullet item that wraps gracefully when the viewport is narrow,
  with styling like **bold** still preserved.

```rust
fn main() {
    println!(\"fenced code is never wrapped\");
}
```";

/// Variant 0 state: a [`RichTextPopup`] over the unwrapped adapter
/// output. `font_role` toggles between the UI font and the editor's
/// monospace font (`f` key) — visual confirmation that
/// `Backend::draw_rich_text_popup_with_font_role` reaches a real
/// second paint path, not just the plain one.
struct PopupState {
    scroll_top: usize,
    font_role: FontRole,
}

impl PopupState {
    fn new() -> Self {
        Self {
            scroll_top: 0,
            font_role: FontRole::Chrome,
        }
    }
}

/// Variant 1 state: a word-wrapped [`ListView`] over the adapter's
/// wrapping entry point.
struct WrapState {
    scroll_offset: usize,
    selected_idx: usize,
}

impl WrapState {
    fn new() -> Self {
        Self {
            scroll_offset: 0,
            selected_idx: 0,
        }
    }
}

pub struct MarkdownDemo {
    popup: PopupState,
    wrap: WrapState,
}

impl MarkdownDemo {
    pub fn new() -> Self {
        Self {
            popup: PopupState::new(),
            wrap: WrapState::new(),
        }
    }

    fn render_popup(&self, backend: &mut dyn Backend, area: Rect) {
        let theme = Theme::default();
        let rendered = render_markdown_to_styled(DOC, &theme);

        let col_w = backend.char_width();
        let row_h = backend.line_height();
        let widest = rendered
            .line_text
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0);
        let content_w = (widest as f32 + 1.0) * col_w;

        let popup = RichTextPopup {
            id: WidgetId::new("gallery:markdown:popup"),
            lines: rendered.lines,
            line_text: rendered.line_text,
            line_scales: rendered.line_scales,
            scroll_top: self.popup.scroll_top,
            max_visible_rows: 16,
            has_focus: true,
            selection: None,
            links: Vec::new(),
            focused_link: None,
            placement: PopupPlacement::Below,
            padding: 1.0,
            fg: None,
            bg: None,
        };

        let anchor_x = area.x + area.width * 0.05;
        let anchor_y = area.y + row_h;
        let measure =
            RichTextPopupMeasure::new(content_w, row_h).with_scale_rows(backend.scales_text_rows());
        let layout = popup.layout(anchor_x, anchor_y, area, measure, |_, start, end| {
            (end - start) as f32 * col_w
        });
        match self.popup.font_role {
            FontRole::Chrome => backend.draw_rich_text_popup(&popup, &layout),
            FontRole::Editor => {
                backend.draw_rich_text_popup_with_font_role(&popup, &layout, FontRole::Editor)
            }
        }
    }

    fn content_cols(area: Rect, backend: &dyn Backend) -> usize {
        let cw = backend.char_width();
        let total_cols = (area.width / cw).floor() as usize;
        total_cols.saturating_sub(2)
    }

    fn render_wrap(&self, backend: &mut dyn Backend, area: Rect) {
        let theme = Theme::default();
        let width = Self::content_cols(area, backend);
        let rendered = render_markdown_to_styled_wrapped(DOC, &theme, width.max(10));

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
            id: WidgetId::new("gallery:markdown:wrap"),
            title: Some(StyledText::plain(" word-wrapped ")),
            items,
            selected_idx: self.wrap.selected_idx.min(n.saturating_sub(1)),
            scroll_offset: self.wrap.scroll_offset,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: true,
        };
        backend.draw_list(area, &list);
    }
}

impl Default for MarkdownDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for MarkdownDemo {
    fn name(&self) -> &'static str {
        "Markdown"
    }

    fn group(&self) -> &'static str {
        "Content"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Popup (unwrapped)", "List (word-wrap)"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        match variant {
            1 => self.render_wrap(backend, area),
            _ => self.render_popup(backend, area),
        }
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        _backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        match variant {
            1 => match event {
                UiEvent::KeyPressed { key, .. } => match key {
                    Key::Char('j') | Key::Named(NamedKey::Down) => {
                        self.wrap.scroll_offset = self.wrap.scroll_offset.saturating_add(1);
                        self.wrap.selected_idx = self.wrap.selected_idx.saturating_add(1);
                        Reaction::Redraw
                    }
                    Key::Char('k') | Key::Named(NamedKey::Up) => {
                        self.wrap.scroll_offset = self.wrap.scroll_offset.saturating_sub(1);
                        self.wrap.selected_idx = self.wrap.selected_idx.saturating_sub(1);
                        Reaction::Redraw
                    }
                    _ => Reaction::Continue,
                },
                _ => Reaction::Continue,
            },
            _ => match event {
                UiEvent::KeyPressed { key, .. } => match key {
                    Key::Named(NamedKey::Down) => {
                        self.popup.scroll_top = self.popup.scroll_top.saturating_add(1);
                        Reaction::Redraw
                    }
                    Key::Named(NamedKey::Up) => {
                        self.popup.scroll_top = self.popup.scroll_top.saturating_sub(1);
                        Reaction::Redraw
                    }
                    Key::Char('f') => {
                        self.popup.font_role = match self.popup.font_role {
                            FontRole::Chrome => FontRole::Editor,
                            FontRole::Editor => FontRole::Chrome,
                        };
                        Reaction::Redraw
                    }
                    _ => Reaction::Continue,
                },
                _ => Reaction::Continue,
            },
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        match variant {
            1 => serde_json::json!({
                "scroll_offset": self.wrap.scroll_offset,
                "selected_idx": self.wrap.selected_idx,
            }),
            _ => serde_json::json!({
                "scroll_top": self.popup.scroll_top,
                "font_role": match self.popup.font_role {
                    FontRole::Chrome => "Chrome",
                    FontRole::Editor => "Editor",
                },
            }),
        }
    }

    fn caps_note(&self, variant: usize, caps: &BackendCaps) -> Option<String> {
        if variant == 0 && !caps.generic_font_families {
            Some(
                "This backend has a fixed cell grid — the 'f' font-role toggle has no \
                 visible effect here."
                    .to_string(),
            )
        } else {
            None
        }
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_font_role_toggles_on_f() {
        let mut demo = MarkdownDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        assert_eq!(demo.popup.font_role, FontRole::Chrome);
        let reaction = demo.handle(
            0,
            &UiEvent::KeyPressed {
                key: Key::Char('f'),
                modifiers: Default::default(),
                repeat: false,
            },
            &mut backend,
            Rect::new(0.0, 0.0, 40.0, 10.0),
        );
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.popup.font_role, FontRole::Editor);
    }

    #[test]
    fn wrap_variant_scroll_down_advances_offset() {
        let mut demo = MarkdownDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        let reaction = demo.handle(
            1,
            &UiEvent::KeyPressed {
                key: Key::Char('j'),
                modifiers: Default::default(),
                repeat: false,
            },
            &mut backend,
            Rect::new(0.0, 0.0, 40.0, 10.0),
        );
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.wrap.scroll_offset, 1);
    }
}
