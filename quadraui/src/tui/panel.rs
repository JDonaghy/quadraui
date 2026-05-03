//! TUI rasteriser for [`crate::Panel`].
//!
//! Paints panel chrome: title bar (background + title text + action
//! glyphs) and content-region background. Content itself is the app's
//! responsibility — it draws into `layout.content_bounds`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{qc, ratatui_color, set_cell};
use crate::primitives::panel::{Panel, PanelLayout, PanelMeasure};
use crate::theme::Theme;

const TUI_TITLE_BAR_HEIGHT: f32 = 1.0;
const TUI_ACTION_BUTTON_WIDTH: f32 = 3.0;

/// Compute the TUI cell-unit layout for a [`Panel`] without painting.
pub fn tui_panel_layout(panel: &Panel, area: Rect) -> PanelLayout {
    let bounds = crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    );
    let measure = PanelMeasure {
        title_bar_height: if panel.title.is_some() {
            TUI_TITLE_BAR_HEIGHT
        } else {
            0.0
        },
        action_button_width: TUI_ACTION_BUTTON_WIDTH,
        content_padding: 0.0,
    };
    panel.layout(bounds, measure)
}

/// Draw a [`Panel`] chrome into `area` on `buf`. Returns the layout
/// for host click dispatch. Content is NOT painted — the app draws
/// into `layout.content_bounds`.
pub fn draw_panel(buf: &mut Buffer, area: Rect, panel: &Panel, theme: &Theme) -> PanelLayout {
    let layout = tui_panel_layout(panel, area);

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let title_bg = panel
        .accent
        .map(ratatui_color)
        .unwrap_or_else(|| qc(theme.separator));
    let title_fg = ratatui_color(theme.foreground);

    // Paint title bar.
    if let Some(tb) = layout.title_bar_bounds {
        let y = tb.y.round() as u16;
        let x_start = tb.x.round() as u16;
        let w = tb.width.round() as u16;

        // Fill title bar background.
        for dx in 0..w {
            set_cell(buf, x_start + dx, y, ' ', title_fg, title_bg);
        }

        // Title text (left-aligned, skip 1 cell padding).
        if let Some(ref title) = panel.title {
            let mut col = x_start + 1;
            let end_x = x_start + w;
            // Leave room for action buttons.
            let text_end = layout
                .visible_actions
                .first()
                .map(|a| a.bounds.x.round() as u16)
                .unwrap_or(end_x);
            for span in &title.spans {
                for ch in span.text.chars() {
                    if col >= text_end {
                        break;
                    }
                    let fg = span.fg.map(ratatui_color).unwrap_or(title_fg);
                    set_cell(buf, col, y, ch, fg, title_bg);
                    col += 1;
                }
            }
        }

        // Action buttons (right-aligned in title bar).
        for va in &layout.visible_actions {
            let ax = va.bounds.x.round() as u16;
            let ay = va.bounds.y.round() as u16;
            let aw = va.bounds.width.round() as u16;
            let action = &panel.actions[va.action_idx];
            let action_bg = if action.is_active {
                ratatui_color(theme.accent_bg)
            } else {
                title_bg
            };

            // Fill action button background.
            for dx in 0..aw {
                set_cell(buf, ax + dx, ay, ' ', title_fg, action_bg);
            }

            // Centre the first glyph of the icon in the button.
            if let Some(glyph) = action.icon.chars().next() {
                let gx = ax + aw / 2;
                set_cell(buf, gx, ay, glyph, title_fg, action_bg);
            }
        }
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::panel::{Panel, PanelAction, PanelHit};
    use crate::types::{StyledSpan, StyledText, WidgetId};

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn titled_panel() -> Panel {
        Panel {
            id: WidgetId::new("panel"),
            title: Some(StyledText {
                spans: vec![StyledSpan::plain("My Panel")],
            }),
            actions: vec![
                PanelAction {
                    id: WidgetId::new("close"),
                    icon: "×".into(),
                    tooltip: "Close".into(),
                    is_active: false,
                },
                PanelAction {
                    id: WidgetId::new("maximize"),
                    icon: "□".into(),
                    tooltip: "Maximize".into(),
                    is_active: false,
                },
            ],
            accent: None,
            collapsed: false,
        }
    }

    #[test]
    fn title_bar_paint_and_click_round_trip() {
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);
        let panel = titled_panel();
        let layout = draw_panel(&mut buf, area, &panel, &Theme::default());

        // Title text should appear starting at column 1.
        assert_eq!(cell_char(&buf, 1, 0), 'M');
        assert_eq!(cell_char(&buf, 2, 0), 'y');

        // Hit the title bar body (column 1) → TitleBar.
        let hit = layout.hit_test(1.5, 0.5);
        assert_eq!(hit, PanelHit::TitleBar(WidgetId::new("panel")));
    }

    #[test]
    fn action_button_paint_and_click_round_trip() {
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);
        let panel = titled_panel();
        let layout = draw_panel(&mut buf, area, &panel, &Theme::default());

        // First action ("close") is rightmost. Its bounds come from layout.
        let close_action = layout
            .visible_actions
            .iter()
            .find(|a| a.id.as_str() == "close")
            .expect("close action should be visible");
        let ax = close_action.bounds.x.round() as u16;
        let aw = close_action.bounds.width.round() as u16;
        let glyph_x = ax + aw / 2;

        // The "×" glyph should be painted at the centre of the button.
        assert_eq!(cell_char(&buf, glyph_x, 0), '×');

        // Hit-test at that glyph position → Action("close").
        let hit = layout.hit_test(glyph_x as f32 + 0.5, 0.5);
        assert_eq!(hit, PanelHit::Action(WidgetId::new("close")));
    }

    #[test]
    fn content_region_paint_and_click_round_trip() {
        let area = Rect::new(0, 0, 20, 10);
        let mut buf = Buffer::empty(area);
        let panel = titled_panel();
        let layout = draw_panel(&mut buf, area, &panel, &Theme::default());

        // Content should start at y=1 (below 1-cell title bar).
        assert!(layout.content_bounds.y >= 1.0);
        assert!(layout.content_bounds.height > 0.0);

        // Hit-test inside content → Content.
        let cx = layout.content_bounds.x + 1.0;
        let cy = layout.content_bounds.y + 1.0;
        let hit = layout.hit_test(cx, cy);
        assert_eq!(hit, PanelHit::Content(WidgetId::new("panel")));
    }

    #[test]
    fn collapsed_panel_has_no_content_hit() {
        let area = Rect::new(0, 0, 20, 10);
        let mut buf = Buffer::empty(area);
        let mut panel = titled_panel();
        panel.collapsed = true;
        let layout = draw_panel(&mut buf, area, &panel, &Theme::default());

        assert_eq!(layout.content_bounds.width, 0.0);
        assert_eq!(layout.content_bounds.height, 0.0);

        // Hit below title bar → Outside (no content region).
        let hit = layout.hit_test(5.0, 2.0);
        assert_eq!(hit, PanelHit::Outside);
    }

    #[test]
    fn no_title_panel_is_all_content() {
        let area = Rect::new(0, 0, 20, 10);
        let mut buf = Buffer::empty(area);
        let panel = Panel {
            id: WidgetId::new("notitle"),
            title: None,
            actions: vec![],
            accent: None,
            collapsed: false,
        };
        let layout = draw_panel(&mut buf, area, &panel, &Theme::default());

        assert!(layout.title_bar_bounds.is_none());
        assert_eq!(layout.content_bounds.x, 0.0);
        assert_eq!(layout.content_bounds.y, 0.0);
        assert_eq!(layout.content_bounds.width, 20.0);
        assert_eq!(layout.content_bounds.height, 10.0);

        let hit = layout.hit_test(5.0, 5.0);
        assert_eq!(hit, PanelHit::Content(WidgetId::new("notitle")));
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let buf_area = Rect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(buf_area);
        let area = Rect::new(0, 0, 0, 0);
        let panel = titled_panel();
        let _layout = draw_panel(&mut buf, area, &panel, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    #[test]
    fn maximize_action_glyph_painted() {
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);
        let panel = titled_panel();
        let layout = draw_panel(&mut buf, area, &panel, &Theme::default());

        let max_action = layout
            .visible_actions
            .iter()
            .find(|a| a.id.as_str() == "maximize")
            .expect("maximize action should be visible");
        let ax = max_action.bounds.x.round() as u16;
        let aw = max_action.bounds.width.round() as u16;
        let glyph_x = ax + aw / 2;

        assert_eq!(cell_char(&buf, glyph_x, 0), '□');

        let hit = layout.hit_test(glyph_x as f32 + 0.5, 0.5);
        assert_eq!(hit, PanelHit::Action(WidgetId::new("maximize")));
    }
}
