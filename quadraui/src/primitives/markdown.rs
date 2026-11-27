//! `Markdown` primitive: a scrollable, read-only markdown viewer, on the
//! widget contract (`Primitive` + `Interactive`).
//!
//! `source` is raw markdown. The widget renders it through the shared
//! markdown adapter ([`render_markdown_to_styled`]) into a
//! [`TextDisplay`] and composes that display: layout, paint, scrolling
//! and scrollbar dragging are the display's own, and its scroll event
//! reaches the host unchanged as `UiEvent::TextDisplay(id, Scrolled { .. })`.
//! `scroll_offset` is app-owned, the same controlled contract as
//! [`TextDisplay::scroll_offset`].
//!
//! Each visible line carrying a link is a sub-region; a left press on one
//! reports `UiEvent::RichTextLinkClicked(id, url)`, the event a
//! `RichTextPopup` link already uses. Hit resolution is line-grained —
//! the first link on a line wins — matching the display's own line hits.

use serde::{Deserialize, Serialize};

use crate::compose::markdown::{render_markdown_to_styled, RenderedMarkdown};
use crate::event::Rect;
use crate::interactive::{Interactive, SubTarget};
use crate::primitive::Primitive;
use crate::primitives::text_display::{
    TextDisplay, TextDisplayEvent, TextDisplayLine, TextDisplayPrimitiveLayout, TextDisplayState,
};
use crate::theme::Theme;
use crate::types::{Decoration, WidgetId};
use crate::Handled;

/// A scrollable, read-only markdown viewer.
///
/// # Examples
///
/// ```
/// use quadraui::{Markdown, WidgetId};
///
/// let md = Markdown::new(WidgetId::new("readme"), "# Title\n\nSee [docs](https://example.com).");
/// assert_eq!(md.scroll_offset, 0);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Markdown {
    pub id: WidgetId,
    /// Raw markdown text.
    pub source: String,
    /// Lines scrolled off the top. The app owns it and applies the
    /// display's `Scrolled` event back onto it.
    #[serde(default)]
    pub scroll_offset: usize,
}

impl Markdown {
    /// A viewer over `source`, scrolled to the top.
    pub fn new(id: WidgetId, source: impl Into<String>) -> Self {
        Self {
            id,
            source: source.into(),
            scroll_offset: 0,
        }
    }

    /// `source` rendered with `theme`'s colours, as the [`TextDisplay`]
    /// this widget composes plus the link table the render found.
    pub fn display(&self, theme: &Theme) -> (TextDisplay, RenderedMarkdown) {
        let rendered = render_markdown_to_styled(&self.source, theme);
        let lines = rendered
            .lines
            .iter()
            .cloned()
            .map(|text| TextDisplayLine {
                spans: text.spans,
                decoration: Decoration::default(),
                timestamp: None,
            })
            .collect();
        let display = TextDisplay {
            id: self.id,
            lines,
            scroll_offset: self.scroll_offset,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: false,
        };
        (display, rendered)
    }
}

/// One visible link: the absolute rect of the line it sits on, and its
/// URL.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkdownLink {
    /// The link's line, in absolute coordinates.
    pub bounds: Rect,
    /// Where the link points.
    pub url: String,
}

/// [`Primitive::Layout`] for [`Markdown`]: the composed display's layout
/// and the visible links.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct MarkdownLayout {
    /// The composed [`TextDisplay`]'s own layout.
    pub display: TextDisplayPrimitiveLayout,
    /// One entry per visible line carrying a link, top to bottom.
    pub links: Vec<MarkdownLink>,
}

impl MarkdownLayout {
    /// The link whose line contains the absolute point `(x, y)`.
    pub fn link_at(&self, x: f32, y: f32) -> Option<&MarkdownLink> {
        self.links
            .iter()
            .find(|link| link.bounds.contains(crate::event::Point::new(x, y)))
    }
}

/// What a [`Markdown`]'s [`crate::interactive::Interactive::handle`]
/// resolves one gesture to.
#[derive(Debug, Clone, PartialEq)]
pub enum MarkdownAction {
    /// A press landed on a link.
    LinkClicked(String),
    /// The composed display reported a scroll (or a key it declined).
    Display(TextDisplayEvent),
}

impl crate::interactive::WidgetAction for MarkdownAction {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            MarkdownAction::LinkClicked(url) => crate::UiEvent::RichTextLinkClicked(id, url),
            MarkdownAction::Display(event) => crate::UiEvent::TextDisplay(id, event),
        }
    }
}

impl Primitive for Markdown {
    type Layout = MarkdownLayout;
    type Event = ();

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn font_role(&self) -> crate::FontRole {
        crate::FontRole::Editor
    }

    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        Primitive::measure(&self.display(&Theme::default()).0, ctx, avail)
    }

    /// The display's own layout over the rendered lines, plus a link rect
    /// for every visible line that carries one. Which lines exist and
    /// where their links sit never depends on colour, so the default
    /// theme renders them here; paint re-renders with the real one.
    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> MarkdownLayout {
        let (display, rendered) = self.display(&Theme::default());
        let display_layout = Primitive::layout(&display, ctx, rect);
        let body = display_layout.body;
        let links = display_layout
            .layout
            .visible_lines
            .iter()
            .filter_map(|visible| {
                let (_, _, url) = rendered
                    .links
                    .iter()
                    .find(|(line_idx, _, _)| *line_idx == visible.line_idx)?;
                Some(MarkdownLink {
                    bounds: Rect::new(
                        body.x + visible.bounds.x,
                        body.y + visible.bounds.y,
                        visible.bounds.width,
                        visible.bounds.height,
                    ),
                    url: url.clone(),
                })
            })
            .collect();
        MarkdownLayout {
            display: display_layout,
            links,
        }
    }

    fn paint(
        &self,
        layout: &MarkdownLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let (display, _) = self.display(env.theme());
        Primitive::paint(&display, &layout.display, surface, env);
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &MarkdownLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let (display, _) = self.display(env.theme());
        Primitive::paint_cells(&display, &layout.display, cells, env);
    }
}

impl Interactive for Markdown {
    type State = TextDisplayState;
    type Action = MarkdownAction;

    /// Links are reached by pointer, so the viewer is no focus stop of
    /// its own.
    fn focus(&self) -> crate::interactive::FocusSpec {
        crate::interactive::FocusSpec::None
    }

    /// One region per visible link line, `SubTarget(i)` for the `i`-th,
    /// in widget-local space.
    fn sub_regions(&self, layout: &MarkdownLayout) -> Vec<(Rect, SubTarget)> {
        let (ox, oy) = (layout.display.area.x, layout.display.area.y);
        layout
            .links
            .iter()
            .enumerate()
            .map(|(i, link)| {
                (
                    Rect::new(
                        link.bounds.x - ox,
                        link.bounds.y - oy,
                        link.bounds.width,
                        link.bounds.height,
                    ),
                    SubTarget(i as u64),
                )
            })
            .collect()
    }

    /// A left press on a link line reports the link; every other event
    /// (the wheel, the scrollbar, keys) is the composed display's.
    fn handle(
        &self,
        state: &mut TextDisplayState,
        event: &crate::interactive::RoutedEvent,
        layout: &MarkdownLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<MarkdownAction> {
        use crate::interactive::{PointerKind, RoutedEvent};

        if let RoutedEvent::Pointer {
            kind: PointerKind::Down(crate::event::MouseButton::Left),
            local,
            ..
        } = event
        {
            let area = layout.display.area;
            if let Some(link) = layout.link_at(area.x + local.x, area.y + local.y) {
                return Handled::Event(MarkdownAction::LinkClicked(link.url.clone()));
            }
        }
        let (display, _) = self.display(&Theme::default());
        match display.handle(state, event, &layout.display, cx) {
            Handled::Event(event) => Handled::Event(MarkdownAction::Display(event)),
            Handled::Consumed => Handled::Consumed,
            Handled::Ignored => Handled::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interactive::{PointerKind, RoutedEvent};
    use crate::primitive::LayoutCtx;
    use crate::{MouseButton, Point};

    fn md() -> Markdown {
        Markdown::new(
            WidgetId::new("md"),
            "# Title\n\nplain line\n\nsee [docs](https://example.com/docs)\n",
        )
    }

    fn run(md: &Markdown, layout: &MarkdownLayout, event: RoutedEvent) -> Handled<MarkdownAction> {
        let mut state = TextDisplayState::default();
        crate::compose::widget_host::with_cx(md.id, crate::primitive::Units::Cells, |cx| {
            md.handle(&mut state, &event, layout, cx)
        })
        .0
    }

    #[test]
    fn a_press_on_a_link_line_reports_its_url_and_elsewhere_does_not() {
        let md = md();
        let layout = Primitive::layout(
            &md,
            &LayoutCtx::new(1.0, 1.0, false),
            Rect::new(0.0, 0.0, 40.0, 10.0),
        );
        assert_eq!(layout.links.len(), 1);
        let link = layout.links[0].bounds;
        let press = |y: f32| RoutedEvent::Pointer {
            kind: PointerKind::Down(MouseButton::Left),
            sub: None,
            local: Point::new(link.x + 1.0, y),
            mods: Default::default(),
        };
        assert_eq!(
            run(&md, &layout, press(link.y)),
            Handled::Event(MarkdownAction::LinkClicked(
                "https://example.com/docs".into()
            ))
        );
        assert!(!matches!(
            run(&md, &layout, press(0.0)),
            Handled::Event(MarkdownAction::LinkClicked(_))
        ));
        assert_eq!(md.sub_regions(&layout).len(), 1);
    }

    #[test]
    fn the_wheel_scrolls_through_the_composed_display() {
        let mut md = md();
        md.source = (0..40).map(|i| format!("line {i}\n\n")).collect();
        let layout = Primitive::layout(
            &md,
            &LayoutCtx::new(1.0, 1.0, false),
            Rect::new(0.0, 0.0, 40.0, 5.0),
        );
        let wheel = RoutedEvent::Wheel {
            dx: 0.0,
            dy: -1.0,
            local: Point::new(1.0, 1.0),
            mods: Default::default(),
        };
        assert_eq!(
            run(&md, &layout, wheel),
            Handled::Event(MarkdownAction::Display(TextDisplayEvent::Scrolled {
                new_offset: 1
            }))
        );
    }
}
