//! `Image` demo — adapted from `quadraui/examples/common/image_app.rs`.
//!
//! Two variants of the same logo at different [`ImageFit`] modes
//! (`Contain` letterboxes within the icon slot; `Cover` fills it,
//! cropping) painted left of a small `File`/`Edit`/`View` menu bar —
//! GTK decodes the real PNG via `gdk_pixbuf`; TUI paints the
//! descriptor's `fallback_text` instead (both through one
//! `Backend::draw_image` call).

use quadraui::{
    Backend, BackendCaps, Color, Image, ImageFit, ImageHit, ImageSource, InteractionState, MenuBar,
    MenuBarHit, MenuBarItem, MouseButton, Reaction, Rect, StatusBar, StatusBarSegment, UiEvent,
    WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("image.rs");

const LOGO_PNG: &[u8] = include_bytes!("../../assets/quadra_logo.png");

// gallery:begin
pub struct ImageDemo {
    menu_bar: MenuBar,
    last_action: Option<String>,
}

impl ImageDemo {
    pub fn new() -> Self {
        Self {
            menu_bar: MenuBar {
                id: WidgetId::new("gallery:image:menu-bar"),
                items: vec![
                    menu_item("gallery:image:file", "&File"),
                    menu_item("gallery:image:edit", "&Edit"),
                    menu_item("gallery:image:view", "&View"),
                ],
                open_item: None,
                focused_item: None,
            },
            last_action: None,
        }
    }

    fn logo(&self, variant: usize) -> Image {
        Image {
            id: WidgetId::new("gallery:image:logo"),
            source: ImageSource::Bytes(LOGO_PNG.to_vec()),
            intrinsic_size: Some((24, 24)),
            fit: if variant == 1 {
                ImageFit::Cover
            } else {
                ImageFit::Contain
            },
            fallback_text: "[Q]".into(),
        }
    }

    fn bar_rects(area: Rect, backend: &dyn Backend) -> (Rect, Rect, Rect) {
        let lh = backend.line_height();
        let full = Rect::new(area.x, area.y, area.width, lh);
        let icon_width = lh * 4.0;
        let icon_rect = Rect::new(full.x, full.y, icon_width, full.height);
        let items_rect = Rect::new(
            full.x + icon_width,
            full.y,
            (full.width - icon_width).max(0.0),
            full.height,
        );
        (full, icon_rect, items_rect)
    }

    fn status(&self) -> StatusBar {
        let msg = match &self.last_action {
            Some(a) => format!(" last: {a} "),
            None => " click a menu item or the logo ".into(),
        };
        StatusBar {
            id: WidgetId::new("gallery:image:status"),
            left_segments: vec![StatusBarSegment {
                text: msg,
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }

    fn status_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y + area.height - lh, area.width, lh)
    }
}

impl Default for ImageDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for ImageDemo {
    fn name(&self) -> &'static str {
        "Image"
    }

    fn group(&self) -> &'static str {
        "Data"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Contain", "Cover"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let (_full, icon_rect, items_rect) = Self::bar_rects(area, backend);
        let _ = backend.draw_image(icon_rect, &self.logo(variant));
        let _ = backend.draw_menu_bar(items_rect, &self.menu_bar);

        let status_rect = Self::status_rect(area, backend);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status(),
            &InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        match event {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let (_full, icon_rect, items_rect) = Self::bar_rects(area, backend);
                let image_layout = self.logo(variant).layout(icon_rect);
                if image_layout.hit_test(position.x, position.y) == ImageHit::Image {
                    self.last_action = Some("clicked logo".into());
                    return Reaction::Redraw;
                }
                let layout = backend.menu_bar_layout(items_rect, &self.menu_bar);
                if let MenuBarHit::Item(idx) = layout.hit_test(position.x, position.y) {
                    self.last_action =
                        Some(format!("activated: {}", self.menu_bar.items[idx].label));
                    return Reaction::Redraw;
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
        serde_json::json!({
            "image": self.logo(variant),
            "last_action": self.last_action,
        })
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        None
    }
}
// gallery:end

fn menu_item(id: &str, label: &str) -> MenuBarItem {
    MenuBarItem {
        id: WidgetId::new(id),
        label: label.into(),
        disabled: false,
        submenu: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_pick_contain_and_cover() {
        let demo = ImageDemo::new();
        assert_eq!(demo.logo(0).fit, ImageFit::Contain);
        assert_eq!(demo.logo(1).fit, ImageFit::Cover);
    }
}

// `TuiBackend` only exists under `feature = "tui"`, and this crate has
// no default features: the `macos` and `win` gates compile these test
// targets with `tui` off, so the module itself must carry the feature.
#[cfg(all(test, feature = "tui"))]
mod tui_tests {
    use super::*;

    #[test]
    fn clicking_the_logo_sets_last_action() {
        let mut demo = ImageDemo::new();
        let mut backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 10.0);
        let (_full, icon_rect, _items_rect) = ImageDemo::bar_rects(area, &backend);
        // `Contain` fit letterboxes the (square) logo within `icon_rect`
        // — click its actual center, not the reserved icon column's
        // edge, which may fall in the letterboxed margin.
        let event = UiEvent::MouseDown {
            button: MouseButton::Left,
            position: quadraui::Point::new(
                icon_rect.x + icon_rect.width / 2.0,
                icon_rect.y + icon_rect.height / 2.0,
            ),
            widget: None,
            modifiers: Default::default(),
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.last_action, Some("clicked logo".into()));
    }
}
