//! `Focus` demo — adapted from `quadraui/examples/common/focus_demo.rs`.
//!
//! Tab / Shift+Tab cycle keyboard focus through two lists and a status
//! row via `compose::FocusRing` — no raw modulo arithmetic, no
//! `FocusRing` reimplementation per app. The focused widget gets
//! `has_focus: true` (border + cursor-visibility, rasteriser-defined);
//! the other two don't. Two variants swap the ring's declared order,
//! showing that Tab visits whatever order the app registered.

use std::cell::{Cell, RefCell};

use quadraui::{
    Backend, BackendCaps, Color, FocusRing, Key, ListItem, ListView, NamedKey, Reaction, Rect,
    StatusBar, StatusBarSegment, StyledText, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("focus.rs");

// gallery:begin
const LEFT_ID: &str = "gallery:focus:left";
const RIGHT_ID: &str = "gallery:focus:right";
const STATUS_ID: &str = "gallery:focus:status";

pub struct FocusDemo {
    /// `RefCell`-wrapped so `render` (`&self`, called every frame with
    /// the *current* variant) can resync the ring's order the moment the
    /// gallery's variant picker changes it, without waiting for the next
    /// `handle()` call — the variant-picker click itself is handled
    /// entirely inside the shell and never reaches `Demo::handle`.
    focus: RefCell<FocusRing>,
    variant: Cell<usize>,
}

impl FocusDemo {
    pub fn new() -> Self {
        Self {
            focus: RefCell::new(Self::ring_for_variant(0)),
            variant: Cell::new(0),
        }
    }

    fn ring_for_variant(variant: usize) -> FocusRing {
        if variant == 1 {
            FocusRing::new(vec![STATUS_ID, RIGHT_ID, LEFT_ID])
        } else {
            FocusRing::new(vec![LEFT_ID, RIGHT_ID, STATUS_ID])
        }
    }

    /// Resync the ring to `variant`'s declared order, preserving the
    /// cycling position only when the order hasn't actually changed
    /// (switching to the variant already active is a no-op).
    fn sync(&self, variant: usize) {
        if self.variant.get() != variant {
            self.variant.set(variant);
            *self.focus.borrow_mut() = Self::ring_for_variant(variant);
        }
    }

    fn list(&self, id: &str, items: &[&str], focused: bool) -> ListView {
        ListView {
            id: WidgetId::new(id),
            title: None,
            items: items
                .iter()
                .map(|name| ListItem {
                    text: StyledText::plain(*name),
                    detail: None,
                    icon: None,
                    decoration: Default::default(),
                })
                .collect(),
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: focused,
            bordered: true,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        }
    }

    fn status_bar(&self, focused: bool) -> StatusBar {
        let (fg, bg) = if focused {
            (Color::rgb(255, 255, 255), Color::rgb(60, 100, 160))
        } else {
            (Color::rgb(220, 220, 220), Color::rgb(40, 40, 60))
        };
        let current = self
            .focus
            .borrow()
            .current()
            .map(|id| id.as_str().to_string())
            .unwrap_or_else(|| "none".to_string());
        StatusBar {
            id: WidgetId::new(STATUS_ID),
            left_segments: vec![StatusBarSegment {
                text: format!(" focus: {current} "),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " Tab / Shift+Tab to move focus ".into(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for FocusDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for FocusDemo {
    fn name(&self) -> &'static str {
        "Focus"
    }

    fn group(&self) -> &'static str {
        "Content"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Reading order", "Reverse order"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        self.sync(variant);
        let lh = backend.line_height();
        let status_h = (lh * 1.5).round();
        let body_h = (area.height - status_h).max(0.0);
        let half_w = (area.width / 2.0).round();

        let ring = self.focus.borrow();
        let left_focused = ring.is_focused(&WidgetId::new(LEFT_ID));
        let right_focused = ring.is_focused(&WidgetId::new(RIGHT_ID));
        let status_focused = ring.is_focused(&WidgetId::new(STATUS_ID));
        drop(ring);

        let left = self.list(LEFT_ID, &["alpha", "bravo", "charlie"], left_focused);
        let right = self.list(RIGHT_ID, &["one", "two", "three"], right_focused);

        backend.draw_list(Rect::new(area.x, area.y, half_w, body_h), &left);
        backend.draw_list(
            Rect::new(area.x + half_w, area.y, area.width - half_w, body_h),
            &right,
        );
        let _ = backend.draw_status_bar_interactive(
            Rect::new(area.x, area.y + body_h, area.width, status_h),
            &self.status_bar(status_focused),
            &quadraui::InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        _backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        self.sync(variant);
        match event {
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                ..
            } => {
                self.focus.borrow_mut().advance();
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::BackTab),
                ..
            } => {
                self.focus.borrow_mut().retreat();
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        self.sync(variant);
        let ring = self.focus.borrow();
        serde_json::json!({
            "order": ring.items().iter().map(|id| id.as_str()).collect::<Vec<_>>(),
            "focused": ring.current().map(|id| id.as_str()),
        })
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        None
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_focuses_the_left_list_first() {
        let demo = FocusDemo::new();
        assert!(demo.focus.borrow().is_focused(&WidgetId::new(LEFT_ID)));
    }

    #[test]
    fn advance_cycles_left_right_status_and_wraps() {
        let demo = FocusDemo::new();
        demo.focus.borrow_mut().advance();
        assert!(demo.focus.borrow().is_focused(&WidgetId::new(RIGHT_ID)));
        demo.focus.borrow_mut().advance();
        assert!(demo.focus.borrow().is_focused(&WidgetId::new(STATUS_ID)));
        demo.focus.borrow_mut().advance();
        assert!(demo.focus.borrow().is_focused(&WidgetId::new(LEFT_ID)));
    }

    #[test]
    fn reverse_variant_starts_at_status() {
        let ring = FocusDemo::ring_for_variant(1);
        assert!(ring.is_focused(&WidgetId::new(STATUS_ID)));
    }

    #[test]
    fn sync_resets_the_ring_when_the_variant_changes() {
        let demo = FocusDemo::new();
        demo.focus.borrow_mut().advance();
        assert!(demo.focus.borrow().is_focused(&WidgetId::new(RIGHT_ID)));
        demo.sync(1);
        assert!(demo.focus.borrow().is_focused(&WidgetId::new(STATUS_ID)));
    }
}
