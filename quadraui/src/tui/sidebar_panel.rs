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
///
/// `resolved_btn` must already have had any `Toolbar::icon_overrides`
/// entry resolved via [`crate::primitives::toolbar::Toolbar::resolve_button_icon`]
/// — see [`tui_sidebar_panel_layout`]'s doc.
fn tui_item_measure(
    resolved_btn: &crate::primitives::toolbar::ToolbarButton,
) -> ToolbarItemMeasure {
    ToolbarItemMeasure::new(super::toolbar::tui_item_width(resolved_btn))
}

/// Compute the TUI cell-unit layout for a `SidebarPanel` without
/// painting.
///
/// `nerd_fonts_enabled` (issue #913 review fix) resolves the embedded
/// toolbar's per-button [`crate::types::Icon`] overrides
/// (`Toolbar::with_icon_override`) the same way [`super::draw_toolbar`]
/// does, so the width this no-paint layout measures — and therefore
/// every hit region `TuiBackend::sidebar_panel_layout` returns — always
/// agrees with what actually painted. A `Toolbar` with no overrides is
/// unaffected by the flag.
pub fn tui_sidebar_panel_layout(
    panel: &SidebarPanel,
    area: Rect,
    nerd_fonts_enabled: bool,
) -> SidebarPanelLayout {
    let bounds = crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    );
    panel.layout(bounds, SidebarPanelMeasure::new(1.0, 1.0), |btn| {
        let resolved = match &panel.toolbar {
            Some(bar) => bar.resolve_button_icon(btn, nerd_fonts_enabled),
            None => std::borrow::Cow::Borrowed(btn),
        };
        tui_item_measure(&resolved)
    })
}

/// Draw a `SidebarPanel` into `area` on `buf`. Returns the layout the
/// host needs to paint its content into `content_bounds` and route
/// clicks via `hit_test`.
///
/// `nerd_fonts_enabled` (issue #913 review fix) is forwarded to the
/// embedded toolbar's rasteriser — see [`super::draw_toolbar`] for its
/// contract. A panel with no toolbar, or a toolbar with no
/// `icon_overrides`, is unaffected by the flag.
#[allow(clippy::too_many_arguments)]
pub fn draw_sidebar_panel(
    buf: &mut Buffer,
    area: Rect,
    panel: &SidebarPanel,
    theme: &Theme,
    hovered_toolbar_id: Option<&WidgetId>,
    pressed_toolbar_id: Option<&WidgetId>,
    nerd_fonts_enabled: bool,
) -> SidebarPanelLayout {
    let layout = tui_sidebar_panel_layout(panel, area, nerd_fonts_enabled);

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
        // Issue #913 review fix: a `SidebarPanel` header toolbar embeds
        // the same `Toolbar` the standalone rasteriser resolves
        // overrides for, so forward the caller's flag instead of a
        // hardcoded `false`.
        let _ = super::draw_toolbar(
            buf,
            tb_rect,
            bar,
            theme,
            hovered_toolbar_id,
            pressed_toolbar_id,
            nerd_fonts_enabled,
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
                focused_index: None,
                icon_overrides: Vec::new(),
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
        let layout =
            draw_sidebar_panel(&mut buf, area, &panel, &Theme::default(), None, None, false);
        // Header at row 0 starts with `[`.
        assert_eq!(cell_char(&buf, 0, 0), '[');
        // Content begins at row 1 and is left blank (host paints).
        assert_eq!(cell_char(&buf, 0, 1), ' ');
        // Layout reports the content rect for the host.
        assert_eq!(layout.content_bounds.y, 1.0);
        assert_eq!(layout.content_bounds.height, 9.0);
    }

    // Parametrized over the area's origin — LESSONS.md "Layout helpers
    // must return coords in the same frame across backends"
    // (quadraui#494). `tui_sidebar_panel_layout` bakes `area.x`/
    // `area.y` straight into `panel.layout`'s returned bounds
    // (absolute frame), so the regression guard is a full
    // paint+hit_test round trip re-run at a non-zero origin.
    fn click_in_header_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 40, 10);
        let mut buf = Buffer::empty(area);
        let panel = panel_with_toolbar();
        let layout =
            draw_sidebar_panel(&mut buf, area, &panel, &Theme::default(), None, None, false);
        match layout.hit_test(origin_x as f32 + 2.0, origin_y as f32) {
            SidebarPanelHit::ToolbarButton(id) => assert_eq!(id.as_str(), "refine"),
            other => panic!("expected ToolbarButton, got {other:?}"),
        }
    }

    #[test]
    fn click_in_header_resolves_to_toolbar_button() {
        click_in_header_round_trip_at(0, 0);
    }

    /// Non-zero-origin regression guard (quadraui#494 / LESSONS.md):
    /// the same header-click round trip must hold when the panel isn't
    /// painted at the screen origin.
    #[test]
    fn click_in_header_resolves_to_toolbar_button_at_nonzero_origin() {
        click_in_header_round_trip_at(7, 13);
    }

    #[test]
    fn click_in_content_returns_content_local_coords() {
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let panel = panel_with_toolbar();
        let layout =
            draw_sidebar_panel(&mut buf, area, &panel, &Theme::default(), None, None, false);
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
        let layout =
            draw_sidebar_panel(&mut buf, area, &panel, &Theme::default(), None, None, false);
        assert!(layout.toolbar_bounds.is_none());
        assert_eq!(layout.content_bounds.y, 0.0);
        assert_eq!(layout.content_bounds.height, 10.0);
    }

    /// Issue #913 review fix: closes the "no test exercises an embedded
    /// toolbar with a registered override" gap — every pre-fix
    /// `nerd_fonts_flag_selects_glyph_or_fallback`-style test targeted
    /// the standalone `Toolbar` path only, so a `SidebarPanel`'s header
    /// toolbar silently never resolved `icon_overrides`. Mirrors
    /// `tui::toolbar::nerd_fonts_flag_selects_glyph_or_fallback` at the
    /// `SidebarPanel` level.
    #[test]
    fn nerd_fonts_flag_selects_glyph_or_fallback_in_embedded_toolbar() {
        use crate::types::Icon;

        let area = Rect::new(0, 0, 40, 10);
        let panel = SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: Some(
                Toolbar::new(
                    WidgetId::new("sb:toolbar"),
                    vec![ToolbarButton::Action {
                        id: WidgetId::new("go"),
                        label: "Go".into(),
                        icon: Some("stale".into()),
                        key_hint: None,
                        enabled: true,
                        is_active: false,
                        tooltip: String::new(),
                    }],
                )
                .with_icon_override(WidgetId::new("go"), Icon::new("\u{f021}", "R")),
            ),
            toolbar_height: None,
        };

        let row_text = |nerd_fonts_enabled: bool| -> String {
            let mut buf = Buffer::empty(area);
            draw_sidebar_panel(
                &mut buf,
                area,
                &panel,
                &Theme::default(),
                None,
                None,
                nerd_fonts_enabled,
            );
            (0..area.width)
                .map(|x| cell_char(&buf, x, 0))
                .collect::<String>()
        };

        assert!(
            row_text(true).contains('\u{f021}'),
            "nerd_fonts_enabled: true should paint the glyph half of the override \
             in a SidebarPanel's embedded toolbar"
        );
        assert!(
            !row_text(true).contains('R'),
            "fallback must not paint when flag is on"
        );
        assert!(
            row_text(false).contains('R'),
            "nerd_fonts_enabled: false should paint the fallback half of the override"
        );
        assert!(
            !row_text(false).contains('\u{f021}'),
            "glyph must not paint when flag is off"
        );
        assert!(
            !row_text(false).contains("stale"),
            "an override must replace the button's own `icon` field entirely, \
             not just supplement it"
        );
    }

    /// Issue #913 review fix: closes the layout/paint width-divergence
    /// gap for a `SidebarPanel`'s embedded `Toolbar` with a registered
    /// `icon_overrides` entry. `tui_sidebar_panel_layout` backs
    /// `TuiBackend::sidebar_panel_layout` — the real hit-test/hover path
    /// — so it must resolve overrides exactly like `draw_sidebar_panel`
    /// does, or a wide Nerd-Font glyph's measured width silently
    /// disagrees with what actually painted.
    #[test]
    fn nerd_fonts_enabled_changes_measured_toolbar_button_width() {
        use crate::types::Icon;

        let area = Rect::new(0, 0, 40, 10);
        let panel = SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: Some(
                Toolbar::new(
                    WidgetId::new("sb:toolbar"),
                    vec![ToolbarButton::Action {
                        id: WidgetId::new("a"),
                        label: "Go".into(),
                        icon: Some("R".into()),
                        key_hint: None,
                        enabled: true,
                        is_active: false,
                        tooltip: String::new(),
                    }],
                )
                // A double-width CJK glyph vs. a single-cell fallback,
                // so the measured cell width must differ.
                .with_icon_override(WidgetId::new("a"), Icon::new("一", "R")),
            ),
            toolbar_height: None,
        };

        let fallback = tui_sidebar_panel_layout(&panel, area, false);
        let glyph = tui_sidebar_panel_layout(&panel, area, true);

        let fallback_w = fallback.toolbar_layout.as_ref().unwrap().visible_items[0]
            .bounds
            .width;
        let glyph_w = glyph.toolbar_layout.as_ref().unwrap().visible_items[0]
            .bounds
            .width;

        assert!(
            glyph_w > fallback_w,
            "a double-width glyph should measure wider than the 1-cell \
             fallback: glyph_w={glyph_w}, fallback_w={fallback_w}"
        );
    }
}
