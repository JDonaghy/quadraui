//! `IconGrid` demo — Finder's "Icons" view / Explorer's "Large icons":
//! a folder of files rendered as a wrapping grid of big icons with a
//! label under each, painted entirely through
//! [`quadraui::DrawsPrimitives::draw_primitive`] — no `Backend::draw_icon_grid`
//! method exists or is needed.
//!
//! Left/Right move by one item (wrapping across rows); Up/Down move by
//! one column count; Home/End jump to the first/last item; Page Up/Page
//! Down move by one viewport's worth of rows; Enter activates the
//! selected tile. A single click selects the tile under the pointer.

use quadraui::{
    icon_grid_move_selection, Backend, BackendCaps, Color, DrawsPrimitives, Icon, IconGrid,
    IconGridHit, IconGridItem, IconGridNav, Key, MouseButton, NamedKey, Reaction, Rect, StatusBar,
    StatusBarSegment, Style, StyledText, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("icon_grid.rs");

/// `Style`'s chrome-metric `token`, floored at the chrome line height on
/// a pixel backend — never shrunk below it — or the plain chrome line
/// height on TUI's fixed cell grid. Mirrors every other demo's own copy
/// of this carve (e.g. `message_list.rs`'s `chrome_strip_height`).
fn chrome_strip_height(backend: &dyn Backend, token: impl Fn(&Style) -> f32) -> f32 {
    let lh = backend.chrome_line_height();
    if lh <= 1.0 {
        backend.snap_height(lh)
    } else {
        backend.snap_height(lh.max(token(&backend.style())))
    }
}

// gallery:begin
pub struct IconGridDemo {
    items: Vec<IconGridItem>,
    selected_idx: usize,
    last_activated: Option<String>,
}

impl IconGridDemo {
    pub fn new() -> Self {
        let files: &[(&str, &str, &str)] = &[
            ("README.md", "markdown", "M"),
            ("src", "folder", "D"),
            ("Cargo.toml", "file", "F"),
            ("main.rs", "file-code", "F"),
            ("assets", "folder", "D"),
            ("logo.png", "file-media", "F"),
            ("CHANGELOG.md", "markdown", "M"),
            ("notes.json", "json", "F"),
            ("archive.zip", "file-zip", "F"),
            ("report.pdf", "file-pdf", "F"),
            ("tests", "folder", "D"),
            ("build.sh", "terminal", "F"),
        ];
        let items = files
            .iter()
            .map(|(name, codicon, fallback)| IconGridItem {
                id: WidgetId::new(format!("gallery:icon-grid:{name}")),
                icon: Icon::codicon(codicon, *fallback).unwrap_or_else(|| Icon::new("", *fallback)),
                image: None,
                label: StyledText::plain(*name),
                detail: None,
                decoration: Default::default(),
            })
            .collect();
        Self {
            items,
            selected_idx: 0,
            last_activated: None,
        }
    }

    /// Tile size this demo asks for, derived from the backend's own
    /// chrome metrics — cell-quantised on TUI (`chrome_line_height() <=
    /// 1.0`), pixel-scaled everywhere else. Set explicitly on
    /// [`IconGrid::tile_size`] (rather than left `None`) so this demo's
    /// own `handle` can compute `columns`/`page_rows` for keyboard nav
    /// without having to reach into [`quadraui::primitive::Primitive::layout`]'s
    /// private default.
    fn tile_size(backend: &dyn Backend) -> (f32, f32) {
        let lh = backend.chrome_line_height();
        if lh <= 1.0 {
            (12.0, 2.0)
        } else {
            (backend.char_width() * 12.0, lh * 3.0)
        }
    }

    fn grid(&self, backend: &dyn Backend) -> IconGrid {
        IconGrid {
            id: WidgetId::new("gallery:icon-grid"),
            items: self.items.clone(),
            selected_idx: self.selected_idx,
            has_focus: true,
            scroll_offset: 0,
            tile_size: Some(Self::tile_size(backend)),
        }
    }

    fn grid_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = chrome_strip_height(backend, |s| s.status_bar_height);
        Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0))
    }

    fn status_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = chrome_strip_height(backend, |s| s.status_bar_height);
        Rect::new(area.x, area.y + area.height - lh, area.width, lh)
    }

    fn status(&self) -> StatusBar {
        let msg = match &self.last_activated {
            Some(name) => format!(" opened {name} "),
            None => " arrows/Home/End/PageUp/PageDown navigate, Enter opens ".to_string(),
        };
        StatusBar {
            id: WidgetId::new("gallery:icon-grid:status"),
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
}

impl Default for IconGridDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for IconGridDemo {
    fn name(&self) -> &'static str {
        "Icon Grid"
    }

    fn group(&self) -> &'static str {
        "Data"
    }

    fn render(&self, _variant: usize, backend: &mut dyn Backend, area: Rect) {
        let rect = Self::grid_rect(area, backend);
        let grid = self.grid(backend);
        backend.draw_primitive(rect, &grid);

        let status_rect = Self::status_rect(area, backend);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status(),
            &quadraui::InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        _variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        let rect = Self::grid_rect(area, backend);
        let (tile_w, tile_h) = Self::tile_size(backend);
        let columns = ((rect.width / tile_w).floor() as usize).max(1);
        let page_rows = ((rect.height / tile_h).floor() as usize).max(1);

        match event {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let grid = self.grid(backend);
                let layout = grid.layout(rect.width, rect.height, tile_w, tile_h);
                match layout.hit_test(position.x - rect.x, position.y - rect.y) {
                    IconGridHit::Item(idx) => {
                        self.selected_idx = idx;
                        Reaction::Redraw
                    }
                    IconGridHit::Empty => Reaction::Continue,
                }
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter),
                ..
            } => {
                if let Some(item) = self.items.get(self.selected_idx) {
                    let label: String = item.label.spans.iter().map(|s| s.text.as_str()).collect();
                    self.last_activated = Some(label);
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(named),
                ..
            } => {
                let nav = match named {
                    NamedKey::Left => IconGridNav::Left,
                    NamedKey::Right => IconGridNav::Right,
                    NamedKey::Up => IconGridNav::Up,
                    NamedKey::Down => IconGridNav::Down,
                    NamedKey::Home => IconGridNav::Home,
                    NamedKey::End => IconGridNav::End,
                    NamedKey::PageUp => IconGridNav::PageUp,
                    NamedKey::PageDown => IconGridNav::PageDown,
                    _ => return Reaction::Continue,
                };
                self.selected_idx = icon_grid_move_selection(
                    self.selected_idx,
                    self.items.len(),
                    columns,
                    page_rows,
                    nav,
                );
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, _variant: usize) -> serde_json::Value {
        serde_json::json!({
            "item_count": self.items.len(),
            "selected_idx": self.selected_idx,
            "last_activated": self.last_activated,
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
    fn right_arrow_advances_the_selection() {
        let mut demo = IconGridDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 20.0);
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Right),
            modifiers: Default::default(),
            repeat: false,
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.selected_idx, 1);
    }

    #[test]
    fn enter_activates_the_selected_item() {
        let mut demo = IconGridDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 20.0);
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Default::default(),
            repeat: false,
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.last_activated.as_deref(), Some("README.md"));
    }
}

#[cfg(all(test, feature = "tui"))]
mod tui_tests {
    use super::*;

    #[test]
    fn clicking_a_tile_selects_it() {
        let mut demo = IconGridDemo::new();
        let mut backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 20.0);
        let rect = IconGridDemo::grid_rect(area, &backend);
        let (tile_w, tile_h) = IconGridDemo::tile_size(&backend);
        let grid = demo.grid(&backend);
        let layout = grid.layout(rect.width, rect.height, tile_w, tile_h);
        let tile1 = layout
            .visible_tiles
            .iter()
            .find(|t| t.item_idx == 1)
            .expect("second tile visible");
        let event = UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: quadraui::Point::new(
                rect.x + tile1.bounds.x + 1.0,
                rect.y + tile1.bounds.y + 1.0,
            ),
            modifiers: Default::default(),
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.selected_idx, 1);
    }
}
