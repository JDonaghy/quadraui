//! `Tooltip` demo — adapted from `quadraui/examples/common/tooltip_demo.rs`.
//!
//! Three variants cycle [`TooltipChrome`]'s `border` vocabulary: `Sides`
//! (bars only), `Full` (closed box, with an optional title row), and
//! `None` (no chrome at all) — the full vocabulary a consumer migrating
//! a bespoke popup needs in order to keep its border and title rather
//! than losing them. `t` toggles the title on the `Full` variant.

use quadraui::{
    Backend, BackendCaps, Color, InteractionState, Key, Reaction, Rect, StatusBar,
    StatusBarSegment, Tooltip, TooltipBorder, TooltipChrome, TooltipMeasure, TooltipPlacement,
    UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("tooltip.rs");

// gallery:begin
const ANCHOR_TEXT: &str = "hover target";
const TOOLTIP_TEXT: &str = "Tooltip content";
const TOOLTIP_TITLE: &str = "Info";

pub struct TooltipDemo {
    show_title: bool,
}

impl TooltipDemo {
    pub fn new() -> Self {
        Self { show_title: true }
    }

    fn border(variant: usize) -> TooltipBorder {
        match variant {
            0 => TooltipBorder::Sides,
            1 => TooltipBorder::Full,
            _ => TooltipBorder::None,
        }
    }

    fn border_label(variant: usize) -> &'static str {
        match variant {
            0 => "Sides",
            1 => "Full",
            _ => "None",
        }
    }

    fn tooltip() -> Tooltip {
        let mut tip = Tooltip::new(WidgetId::new("gallery:tooltip:tip"), TOOLTIP_TEXT);
        tip.placement = TooltipPlacement::Bottom;
        tip
    }

    /// `Full` needs a top and bottom border row on top of the one content
    /// row; `Sides`/`None` reserve no border rows at all (see the
    /// contract note on `TooltipMeasure`).
    fn rows(variant: usize) -> f32 {
        if variant == 1 {
            3.0
        } else {
            1.0
        }
    }

    fn status_bar(&self, variant: usize) -> StatusBar {
        StatusBar {
            id: WidgetId::new("gallery:tooltip:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(
                    " border: {} | title: {} ",
                    Self::border_label(variant),
                    self.show_title
                ),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " t=toggle title (Full only) ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn anchor_bar() -> StatusBar {
        StatusBar {
            id: WidgetId::new("gallery:tooltip:anchor"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {ANCHOR_TEXT} "),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(37, 37, 38),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }
}

impl Default for TooltipDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for TooltipDemo {
    fn name(&self) -> &'static str {
        "Tooltip"
    }

    fn group(&self) -> &'static str {
        "Overlays"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Sides", "Full", "None"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();
        let cw = backend.char_width();

        // Anchor bar at the top — the element the tooltip "describes".
        let anchor = Rect::new(area.x, area.y, area.width, lh);
        let _ = backend.draw_status_bar_interactive(
            anchor,
            &Self::anchor_bar(),
            &InteractionState::new(),
        );

        // Status bar at the bottom shows the current border/title choice
        // plus the key hint.
        let status_rect = Rect::new(area.x, area.y + area.height - lh, area.width, lh);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(variant),
            &InteractionState::new(),
        );

        // Tooltip renders between the two bars, below the anchor.
        let clamp = Rect::new(
            area.x,
            area.y + lh,
            area.width,
            (area.height - 2.0 * lh).max(0.0),
        );
        let tooltip = Self::tooltip();
        let measure = TooltipMeasure::new(
            cw * (TOOLTIP_TEXT.chars().count() as f32 + 4.0),
            lh * Self::rows(variant),
        );
        let layout = tooltip.layout(anchor, clamp, measure, lh);
        let mut chrome = TooltipChrome::new(Self::border(variant));
        if self.show_title && variant == 1 {
            chrome.title = Some(TOOLTIP_TITLE.to_string());
        }
        backend.draw_tooltip_with_chrome(&tooltip, &layout, &chrome);
    }

    fn handle(
        &mut self,
        _variant: usize,
        event: &UiEvent,
        _backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('t'),
                ..
            } => {
                self.show_title = !self.show_title;
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        serde_json::json!({
            "border": Self::border_label(variant),
            "title_shown": self.show_title && variant == 1,
        })
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        // `draw_tooltip_with_chrome` is a required `Backend` trait method
        // with no no-op default — every backend paints the same chrome
        // vocabulary, so there's no capability gap to report here.
        None
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn border_label_matches_each_variant() {
        assert_eq!(TooltipDemo::border_label(0), "Sides");
        assert_eq!(TooltipDemo::border_label(1), "Full");
        assert_eq!(TooltipDemo::border_label(2), "None");
    }

    #[test]
    fn toggling_title_flips_the_flag() {
        let mut demo = TooltipDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        let area = Rect::new(0.0, 0.0, 40.0, 10.0);
        assert!(demo.show_title);

        let key_t = UiEvent::KeyPressed {
            key: Key::Char('t'),
            modifiers: quadraui::Modifiers::default(),
            repeat: false,
        };
        demo.handle(1, &key_t, &mut backend, area);
        assert!(!demo.show_title);
        assert_eq!(demo.data(1)["title_shown"], false);

        demo.handle(1, &key_t, &mut backend, area);
        assert!(demo.show_title);
        assert_eq!(demo.data(1)["title_shown"], true);
    }
}
