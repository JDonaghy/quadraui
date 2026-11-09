//! TUI rasteriser for [`crate::Panel`].
//!
//! Paints panel chrome: title bar (background + title text + action
//! glyphs) and content-region background. Content itself is the app's
//! responsibility — it draws into `layout.content_bounds`.
//!
//! Title bar painting routes through [`Panel::header_toolbar`]'s mapped
//! [`crate::Toolbar`] header: the title/spacer/action positions come
//! from [`Toolbar::layout_with_options`] rather than this module
//! re-deriving them, so the only TUI-specific work left here is
//! choosing how each [`ToolbarButton`] kind paints onto cells — a
//! `Label` as plain text, an `Action` as a single glyph centred in its
//! button (matching this crate's own chrome everywhere else a
//! `PanelAction` paints), never the bracketed `[ icon label ]` style
//! [`super::draw_toolbar`] uses for a standalone `Toolbar`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{qc, ratatui_color, set_cell};
// `Panel` is `#[deprecated]` (use `SidebarPanel` + `Toolbar` instead); this
// whole module is `Panel`'s own TUI rasteriser plumbing.
#[allow(deprecated)]
use crate::primitives::panel::{Panel, PanelLayout, PanelMeasure};
use crate::primitives::toolbar::{ToolbarButton, ToolbarItemMeasure, ToolbarLayoutOptions};
use crate::theme::Theme;

const TUI_TITLE_BAR_HEIGHT: f32 = 1.0;
const TUI_ACTION_BUTTON_WIDTH: f32 = 3.0;

/// TUI chrome measurements for `panel` — shared by [`tui_panel_layout`]
/// (hit-test layout, via [`Panel::layout`]) and [`draw_panel`] (paint,
/// via [`Panel::header_toolbar`]) so both agree on the exact same
/// title-bar height and action-button width.
#[allow(deprecated)] // `Panel` is `#[deprecated]`; this fn is its own plumbing.
fn tui_panel_measure(panel: &Panel) -> PanelMeasure {
    PanelMeasure {
        title_bar_height: if panel.title.is_some() {
            TUI_TITLE_BAR_HEIGHT
        } else {
            0.0
        },
        action_button_width: TUI_ACTION_BUTTON_WIDTH,
        content_padding: 0.0,
    }
}

/// Compute the TUI cell-unit layout for a [`Panel`] without painting.
#[allow(deprecated)] // `Panel` is `#[deprecated]`; this fn is its own plumbing.
pub fn tui_panel_layout(panel: &Panel, area: Rect) -> PanelLayout {
    let bounds = crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    );
    panel.layout(bounds, tui_panel_measure(panel))
}

/// Draw a [`Panel`] chrome into `area` on `buf`. Returns the layout
/// for host click dispatch. Content is NOT painted — the app draws
/// into `layout.content_bounds`.
#[allow(deprecated)] // `Panel` is `#[deprecated]`; this fn is its own plumbing.
pub fn draw_panel(buf: &mut Buffer, area: Rect, panel: &Panel, theme: &Theme) -> PanelLayout {
    let layout = tui_panel_layout(panel, area);

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let bounds = crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    );
    let measure = tui_panel_measure(panel);

    let title_bg = panel
        .accent
        .map(ratatui_color)
        .unwrap_or_else(|| qc(theme.separator));
    let title_fg = ratatui_color(theme.foreground);

    // Paint title bar, when present — `header_toolbar` returns `None`
    // in exactly the cases `layout.title_bar_bounds` is `None` too.
    if let (Some(tb), Some(header)) = (
        layout.title_bar_bounds,
        panel.header_toolbar(bounds, measure),
    ) {
        let y = tb.y.round() as u16;
        let x_start = tb.x.round() as u16;
        let w = tb.width.round() as u16;

        // Fill the whole title bar with its background first; the
        // per-item loop below only overwrites action-button cells.
        for dx in 0..w {
            set_cell(buf, x_start + dx, y, ' ', title_fg, title_bg);
        }

        // Lay the mapped header out with the same per-kind measure
        // `Panel::header_toolbar`'s own doc promises matches
        // `Panel::layout`'s action bounds: a fixed `action_button_width`
        // per `Action`, the `Label`'s own text width otherwise. This is
        // also what naturally reproduces the old "clip the title before
        // it reaches the action buttons" behaviour — `Toolbar`'s own
        // leading/trailing collision rule, not a hand-rolled `text_end`.
        let header_layout = header.layout_with_options(
            tb.x,
            tb.y,
            tb.width,
            tb.height,
            ToolbarLayoutOptions::default(),
            |btn| match btn {
                ToolbarButton::Action { .. } => {
                    ToolbarItemMeasure::new(measure.action_button_width)
                }
                ToolbarButton::Label { text, .. } => {
                    ToolbarItemMeasure::new(text.chars().count() as f32)
                }
                _ => ToolbarItemMeasure::new(0.0),
            },
        );

        for vis in &header_layout.visible_items {
            let vx = vis.bounds.x.round() as u16;
            let vw = vis.bounds.width.round() as u16;
            if vw == 0 {
                continue;
            }
            match &header.buttons[vis.item_idx] {
                ToolbarButton::Label { text, fg } => {
                    let color = fg.map(ratatui_color).unwrap_or(title_fg);
                    for (col, ch) in (0u16..vw).zip(text.chars()) {
                        set_cell(buf, vx + col, y, ch, color, title_bg);
                    }
                }
                ToolbarButton::Action {
                    label, is_active, ..
                } => {
                    let action_bg = if *is_active {
                        ratatui_color(theme.accent_bg)
                    } else {
                        title_bg
                    };

                    // Fill action button background.
                    for dx in 0..vw {
                        set_cell(buf, vx + dx, y, ' ', title_fg, action_bg);
                    }

                    // Centre the first glyph of the icon (mapped onto
                    // the button's `label`) in the button.
                    if let Some(glyph) = label.chars().next() {
                        let gx = vx + vw / 2;
                        set_cell(buf, gx, y, glyph, title_fg, action_bg);
                    }
                }
                // Already painted by the whole-bar background fill
                // above; a `Toolbar` built by `header_toolbar` never
                // contains a `Separator`.
                ToolbarButton::Spacer | ToolbarButton::Separator => {}
            }
        }
    }

    layout
}

#[cfg(test)]
#[allow(deprecated)] // `Panel`/`PanelAction` are `#[deprecated]`; these tests cover their TUI rasteriser directly.
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

    /// Shared body for the title-bar paint↔click round trip, run at both
    /// the origin and a non-zero `area` origin. `tui_panel_layout` bakes
    /// `area.x`/`area.y` straight into the returned bounds (absolute frame,
    /// matching the GTK/macOS twins), so both the painted glyph position
    /// AND the `hit_test` coordinates must shift together with the origin.
    fn title_bar_paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 30, 10);
        let mut buf = Buffer::empty(Rect::new(0, 0, origin_x + 30, origin_y + 10));
        let panel = titled_panel();
        let layout = draw_panel(&mut buf, area, &panel, &Theme::default());

        // Title text should appear starting at column area.x + 1.
        assert_eq!(cell_char(&buf, area.x + 1, area.y), 'M');
        assert_eq!(cell_char(&buf, area.x + 2, area.y), 'y');

        // Hit the title bar body (column area.x + 1) → TitleBar.
        let hit = layout.hit_test(area.x as f32 + 1.5, area.y as f32 + 0.5);
        assert_eq!(hit, PanelHit::TitleBar(WidgetId::new("panel")));
    }

    #[test]
    fn title_bar_paint_and_click_round_trip() {
        title_bar_paint_and_click_round_trip_at(0, 0);
    }

    /// Non-zero-origin regression guard: same round trip,
    /// painted at a shifted `area` origin.
    #[test]
    fn title_bar_paint_and_click_round_trip_at_nonzero_origin() {
        title_bar_paint_and_click_round_trip_at(7, 13);
    }

    fn action_button_paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 30, 10);
        let mut buf = Buffer::empty(Rect::new(0, 0, origin_x + 30, origin_y + 10));
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
        assert_eq!(cell_char(&buf, glyph_x, area.y), '×');

        // Hit-test at that glyph position → Action("close").
        let hit = layout.hit_test(glyph_x as f32 + 0.5, area.y as f32 + 0.5);
        assert_eq!(hit, PanelHit::Action(WidgetId::new("close")));
    }

    #[test]
    fn action_button_paint_and_click_round_trip() {
        action_button_paint_and_click_round_trip_at(0, 0);
    }

    /// Non-zero-origin regression guard.
    #[test]
    fn action_button_paint_and_click_round_trip_at_nonzero_origin() {
        action_button_paint_and_click_round_trip_at(7, 13);
    }

    fn content_region_paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 20, 10);
        let mut buf = Buffer::empty(Rect::new(0, 0, origin_x + 20, origin_y + 10));
        let panel = titled_panel();
        let layout = draw_panel(&mut buf, area, &panel, &Theme::default());

        // Content should start at area.y + 1 (below 1-cell title bar).
        assert!(layout.content_bounds.y >= area.y as f32 + 1.0);
        assert!(layout.content_bounds.height > 0.0);

        // Hit-test inside content → Content.
        let cx = layout.content_bounds.x + 1.0;
        let cy = layout.content_bounds.y + 1.0;
        let hit = layout.hit_test(cx, cy);
        assert_eq!(hit, PanelHit::Content(WidgetId::new("panel")));
    }

    #[test]
    fn content_region_paint_and_click_round_trip() {
        content_region_paint_and_click_round_trip_at(0, 0);
    }

    /// Non-zero-origin regression guard.
    #[test]
    fn content_region_paint_and_click_round_trip_at_nonzero_origin() {
        content_region_paint_and_click_round_trip_at(7, 13);
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

    /// Pins the one visual divergence the `header_toolbar` seam introduces:
    /// a multi-span title loses its per-span `fg` once mapped onto the
    /// header's single [`crate::primitives::toolbar::ToolbarButton::Label`]
    /// (`Panel::header_toolbar` flattens `title` to one plain string), so
    /// every span paints in `theme.foreground` instead of its own color.
    #[test]
    fn multi_span_title_fg_is_flattened_to_theme_foreground() {
        use crate::types::Color;

        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);
        let mut panel = titled_panel();
        let red = Color::rgb(200, 0, 0);
        panel.title = Some(StyledText {
            spans: vec![StyledSpan::with_fg("Red", red), StyledSpan::plain(" Plain")],
        });
        let theme = Theme::default();
        draw_panel(&mut buf, area, &panel, &theme);

        // Every title cell paints in `theme.foreground`, regardless of
        // the span's own `fg`.
        assert_eq!(buf[(1, 0)].fg, ratatui_color(theme.foreground));
        assert_ne!(buf[(1, 0)].fg, ratatui_color(red));
    }
}
