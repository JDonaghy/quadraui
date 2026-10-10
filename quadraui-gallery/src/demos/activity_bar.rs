//! `ActivityBar` demo — adapted from `quadraui/examples/common/activity_nav.rs`
//! and `activity_style_demo.rs`.
//!
//! Two variants show the two ways an active item can be highlighted:
//! a left-edge accent line (`ActivityBar::active_accent`), or a VS-Code-style
//! soft row fill requested through `Backend::draw_activity_bar_with_style` —
//! never a field on `ActivityBar` itself.

use quadraui::{
    ActivityBar, ActivityBarStyle, ActivityItem, Backend, BackendCaps, Color, Reaction, Rect,
    UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("activity_bar.rs");

// gallery:begin
const LABELS: [&str; 3] = ["Explorer", "Search", "Source Control"];
const ICONS: [&str; 3] = ["E", "S", "G"];

fn item_id(variant: usize, i: usize) -> WidgetId {
    WidgetId::new(format!("gallery:activity-bar:{variant}:{i}"))
}

pub struct ActivityBarDemo {
    active: [usize; 2],
    last_action: String,
}

impl ActivityBarDemo {
    pub fn new() -> Self {
        Self {
            active: [0, 0],
            last_action: "ready".into(),
        }
    }

    fn bar(&self, variant: usize) -> ActivityBar {
        let items = ICONS
            .iter()
            .enumerate()
            .map(|(i, icon)| ActivityItem {
                id: item_id(variant, i),
                icon: (*icon).into(),
                tooltip: LABELS[i].to_string(),
                is_active: i == self.active[variant],
                is_keyboard_selected: false,
            })
            .collect();
        ActivityBar {
            id: WidgetId::new(format!("gallery:activity-bar:{variant}")),
            top_items: items,
            bottom_items: vec![],
            // Variant 0 paints the accent line; variant 1 relies solely on
            // the row-fill style below, so this is `None` there.
            active_accent: if variant == 0 {
                Some(Color::rgb(100, 150, 255))
            } else {
                None
            },
            selection_bg: None,
            is_keyboard_focused: false,
        }
    }

    /// Only meaningful for variant 1 — the VS-Code-style soft chip fill.
    fn style(&self) -> ActivityBarStyle {
        ActivityBarStyle::new().with_active_bg(Color::rgb(49, 50, 51))
    }

    fn activate(&mut self, variant: usize, idx: usize) {
        self.active[variant] = idx;
        self.last_action = format!("activated: {}", LABELS[idx]);
    }
}

impl Default for ActivityBarDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for ActivityBarDemo {
    fn name(&self) -> &'static str {
        "Activity Bar"
    }

    fn group(&self) -> &'static str {
        "Chrome"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Accent line", "Row fill (VS Code)"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let rect = area;
        let bar = self.bar(variant);
        if variant == 0 {
            let _ = backend.draw_activity_bar(rect, &bar, None);
        } else {
            let _ = backend.draw_activity_bar_with_style(rect, &bar, None, &self.style());
        }
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        match event {
            UiEvent::MouseDown { position, .. } => {
                let rect = area;
                if position.x < rect.x
                    || position.x >= rect.x + rect.width
                    || position.y < rect.y
                    || position.y >= rect.y + rect.height
                {
                    return Reaction::Continue;
                }
                let bar = self.bar(variant);
                let hits = backend.activity_bar_layout(rect, &bar);
                let rel_y = position.y - rect.y;
                for hit in &hits {
                    if rel_y >= hit.y_start && rel_y < hit.y_end {
                        if let Some(idx) =
                            (0..LABELS.len()).find(|&i| hit.id == item_id(variant, i))
                        {
                            self.activate(variant, idx);
                            return Reaction::Redraw;
                        }
                    }
                }
                Reaction::Continue
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        let mut value = serde_json::to_value(self.bar(variant)).unwrap_or(serde_json::Value::Null);
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "last_action".into(),
                serde_json::Value::String(self.last_action.clone()),
            );
        }
        value
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
    fn clicking_an_item_activates_it() {
        let mut demo = ActivityBarDemo::new();
        assert_eq!(demo.active[0], 0);
        demo.activate(0, 2);
        assert_eq!(demo.active[0], 2);
        assert!(demo.last_action.contains("Source Control"));
    }
}
