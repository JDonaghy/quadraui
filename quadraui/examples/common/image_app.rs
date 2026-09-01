//! Image + MenuBar leading-icon-slot `AppLogic` ([`tui_image`] /
//! [`gtk_image`]).
//!
//! Demonstrates the two halves of #662 together, since that's the real
//! motivating scenario (vimcode wanting its app logo left of `File`,
//! VS-Code style):
//!
//! - [`quadraui::Image`] painted through `Backend::draw_image` — GTK
//!   decodes and paints the real `quadra_logo.png` asset via
//!   `gdk_pixbuf`; TUI paints the descriptor's `fallback_text` instead
//!   (`Backend::draw_image` cannot rasterise pixels on TUI — see that
//!   trait method's doc comment for why that's a deliberate
//!   `ImagePaintResult::Unsupported`, not a silent no-op).
//! - A leading icon slot on the menu bar: the bar's rect is narrowed by
//!   the icon's width before *both* `Backend::draw_menu_bar` (paint) and
//!   `Backend::menu_bar_layout` (click-routing) — see [`ImageApp::bar_rects`],
//!   computed once so paint and hit-test can never disagree. This demo
//!   narrows the rect itself rather than calling
//!   [`quadraui::MenuBar::layout_with_leading`] directly, because
//!   `Backend::draw_menu_bar` has no leading-width parameter to hand it
//!   to — that per-app narrowing is exactly the offset math
//!   `layout_with_leading` exists to centralize (see its doc comment
//!   and `primitives::menu_bar::tests` for the primitive-level proof).
//!
//! Controls:
//! - click a menu item   show which one was activated
//! - q / Esc              quit

use quadraui::{
    AppLogic, Backend, Color, Image, ImageFit, ImageSource, Key, MenuBar, MenuBarHit, MenuBarItem,
    MouseButton, NamedKey, Reaction, Rect, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

const LOGO_PNG: &[u8] = include_bytes!("../assets/quadra_logo.png");

pub struct ImageApp {
    menu_bar: MenuBar,
    last_action: Option<String>,
}

impl ImageApp {
    pub fn new() -> Self {
        Self {
            menu_bar: MenuBar {
                id: WidgetId::new("menu-bar"),
                items: vec![
                    menu_item("file", "&File"),
                    menu_item("edit", "&Edit"),
                    menu_item("view", "&View"),
                ],
                open_item: None,
                focused_item: None,
            },
            last_action: None,
        }
    }

    fn logo(&self) -> Image {
        Image {
            id: WidgetId::new("logo"),
            source: ImageSource::Bytes(LOGO_PNG.to_vec()),
            intrinsic_size: Some((24, 24)),
            fit: ImageFit::Contain,
            fallback_text: "[Q]".into(),
        }
    }

    /// `(full bar rect, icon rect, item rect)` — the icon reserves
    /// `4 * line_height` at the bar's left edge (wide enough for TUI's
    /// `[Q]` fallback text to fully paint, not just its first cell);
    /// everything else is the menu items' rect. Both [`Self::render`]
    /// and [`Self::handle`] call this rather than each re-deriving the
    /// narrowed rect independently, so paint and click-routing always
    /// agree (the bug class [`quadraui::MenuBar::layout_with_leading`]'s
    /// doc comment calls out).
    fn bar_rects(&self, backend: &dyn Backend) -> (Rect, Rect, Rect) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let full = Rect::new(0.0, 0.0, viewport.width, lh);
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

    fn status_bar(&self) -> StatusBar {
        let msg = match &self.last_action {
            Some(a) => format!(" last: {a} "),
            None => " click a menu item — q to quit ".into(),
        };
        StatusBar {
            id: WidgetId::new("status"),
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

impl Default for ImageApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for ImageApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let (_full, icon_rect, items_rect) = self.bar_rects(backend);

        let _ = backend.draw_image(icon_rect, &self.logo());
        let _ = backend.draw_menu_bar(items_rect, &self.menu_bar);

        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar(status_rect, &self.status_bar(), None, None);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let (_full, _icon_rect, items_rect) = self.bar_rects(backend);
                let layout = backend.menu_bar_layout(items_rect, &self.menu_bar);
                if let MenuBarHit::Item(idx) = layout.hit_test(position.x, position.y) {
                    self.last_action =
                        Some(format!("activated: {}", self.menu_bar.items[idx].label));
                }
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}

fn menu_item(id: &str, label: &str) -> MenuBarItem {
    MenuBarItem {
        id: WidgetId::new(id),
        label: label.into(),
        disabled: false,
        submenu: None,
    }
}
