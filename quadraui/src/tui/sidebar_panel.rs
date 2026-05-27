//! TUI rasteriser for [`crate::primitives::sidebar_panel::SidebarPanel`].
//!
//! Paints the toolbar header slot via [`super::draw_toolbar`]. The
//! content region (`SidebarPanelLayout.content_bounds`) is **not**
//! painted — the host is responsible for drawing its tree / list /
//! form / whatever into that rect. Same contract as [`super::draw_panel`].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::primitives::sidebar_panel::{SidebarPanel, SidebarPanelLayout, SidebarPanelMeasure};
use crate::primitives::toolbar::ToolbarItemMeasure;
use crate::theme::Theme;
use crate::types::WidgetId;

/// Cell measurement for the nested toolbar. Mirrors
/// [`super::toolbar::tui_item_width`]; pulled into a free function so
/// `sidebar_panel_layout` (no paint) can reuse it without forcing
/// `tui_item_width` to leave the `toolbar` submodule.
fn tui_item_measure(btn: &crate::primitives::toolbar::ToolbarButton) -> ToolbarItemMeasure {
    ToolbarItemMeasure::new(super::toolbar::tui_item_width(btn))
}

/// Compute the TUI cell-unit layout for a `SidebarPanel` without
/// painting.
pub fn tui_sidebar_panel_layout(panel: &SidebarPanel, area: Rect) -> SidebarPanelLayout {
    let bounds = crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    );
    panel.layout(bounds, SidebarPanelMeasure::new(1.0, 1.0), tui_item_measure)
}

/// Draw a `SidebarPanel` into `area` on `buf`. Returns the layout the
/// host needs to paint its content into `content_bounds` and route
/// clicks via `hit_test`.
pub fn draw_sidebar_panel(
    buf: &mut Buffer,
    area: Rect,
    panel: &SidebarPanel,
    theme: &Theme,
    hovered_toolbar_id: Option<&WidgetId>,
    pressed_toolbar_id: Option<&WidgetId>,
) -> SidebarPanelLayout {
    let layout = tui_sidebar_panel_layout(panel, area);

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    if let (Some(bar), Some(tb_layout), Some(tb_bounds)) = (
        &panel.toolbar,
        layout.toolbar_layout.as_ref(),
        layout.toolbar_bounds,
    ) {
        let _ = tb_layout; // re-derived inside draw_toolbar
        let tb_rect = Rect::new(
            tb_bounds.x.round() as u16,
            tb_bounds.y.round() as u16,
            tb_bounds.width.round() as u16,
            tb_bounds.height.round() as u16,
        );
        let _ = super::draw_toolbar(
            buf,
            tb_rect,
            bar,
            theme,
            hovered_toolbar_id,
            pressed_toolbar_id,
        );
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::sidebar_panel::SidebarPanelHit;
    use crate::primitives::toolbar::{Toolbar, ToolbarButton};
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn panel_with_toolbar() -> SidebarPanel {
        SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: Some(Toolbar {
                id: WidgetId::new("sb:toolbar"),
                buttons: vec![ToolbarButton::Action {
                    id: WidgetId::new("refine"),
                    label: "Refine".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                }],
                bg: None,
            }),
            toolbar_height: None,
        }
    }

    #[test]
    fn paints_toolbar_in_top_row() {
        // 1-cell header slot at row 0.
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let panel = panel_with_toolbar();
        let layout = draw_sidebar_panel(&mut buf, area, &panel, &Theme::default(), None, None);
        // Header at row 0 starts with `[`.
        assert_eq!(cell_char(&buf, 0, 0), '[');
        // Content begins at row 1 and is left blank (host paints).
        assert_eq!(cell_char(&buf, 0, 1), ' ');
        // Layout reports the content rect for the host.
        assert_eq!(layout.content_bounds.y, 1.0);
        assert_eq!(layout.content_bounds.height, 9.0);
    }

    #[test]
    fn click_in_header_resolves_to_toolbar_button() {
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let panel = panel_with_toolbar();
        let layout = draw_sidebar_panel(&mut buf, area, &panel, &Theme::default(), None, None);
        match layout.hit_test(2.0, 0.0) {
            SidebarPanelHit::ToolbarButton(id) => assert_eq!(id.as_str(), "refine"),
            other => panic!("expected ToolbarButton, got {other:?}"),
        }
    }

    #[test]
    fn click_in_content_returns_content_local_coords() {
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let panel = panel_with_toolbar();
        let layout = draw_sidebar_panel(&mut buf, area, &panel, &Theme::default(), None, None);
        match layout.hit_test(5.0, 4.0) {
            // Content origin y = 1, so local y = 4 - 1 = 3.
            SidebarPanelHit::Content { x, y } => {
                assert_eq!(x, 5.0);
                assert_eq!(y, 3.0);
            }
            other => panic!("expected Content, got {other:?}"),
        }
    }

    #[test]
    fn no_toolbar_gives_full_rect_to_content() {
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let panel = SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: None,
            toolbar_height: None,
        };
        let layout = draw_sidebar_panel(&mut buf, area, &panel, &Theme::default(), None, None);
        assert!(layout.toolbar_bounds.is_none());
        assert_eq!(layout.content_bounds.y, 0.0);
        assert_eq!(layout.content_bounds.height, 10.0);
    }
}
