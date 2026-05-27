//! macOS rasteriser for [`crate::primitives::sidebar_panel::SidebarPanel`].
//!
//! Paints the optional toolbar header by delegating to
//! [`super::toolbar::draw_toolbar`]. The content rect is **not**
//! painted — returned in `SidebarPanelLayout.content_bounds` for the
//! host to draw into.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::primitives::sidebar_panel::{SidebarPanel, SidebarPanelLayout, SidebarPanelMeasure};
use crate::primitives::toolbar::ToolbarItemMeasure;
use crate::theme::Theme;
use crate::types::WidgetId;

use super::text::measure_text;

/// Compute the macOS pixel-unit layout for a `SidebarPanel`. `font`
/// is required for accurate text measurement.
pub fn mac_sidebar_panel_layout(
    panel: &SidebarPanel,
    font: &CTFont,
    line_height: f64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> SidebarPanelLayout {
    let bounds = crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32);
    panel.layout(
        bounds,
        SidebarPanelMeasure::new(line_height as f32, 8.0),
        |btn| ToolbarItemMeasure::new(item_width_px(font, btn) as f32),
    )
}

fn item_width_px(font: &CTFont, btn: &crate::primitives::toolbar::ToolbarButton) -> f64 {
    use crate::primitives::toolbar::ToolbarButton;
    const ACTION_H_PAD: f64 = 8.0;
    const SEPARATOR_PX: f64 = 12.0;
    match btn {
        ToolbarButton::Action {
            label,
            icon,
            key_hint,
            ..
        } => {
            let mut s = String::new();
            if let Some(icon) = icon {
                s.push_str(icon);
                s.push(' ');
            }
            s.push_str(label);
            if let Some(hint) = key_hint {
                s.push_str(" (");
                s.push_str(hint);
                s.push(')');
            }
            let (w, _) = measure_text(font, &s);
            w + 2.0 * ACTION_H_PAD
        }
        ToolbarButton::Separator => SEPARATOR_PX,
        ToolbarButton::Label { text, .. } => measure_text(font, text).0,
    }
}

/// Paint a `SidebarPanel` onto `ctx`. Returns the resolved layout
/// for the host to paint content into and route clicks.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call (typical: the frame-scope pointer stashed on
/// [`super::MacBackend`]).
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_sidebar_panel(
    ctx: CGContextRef,
    font: &CTFont,
    line_height: f64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    panel: &SidebarPanel,
    theme: &Theme,
    hovered_toolbar_id: Option<&WidgetId>,
    pressed_toolbar_id: Option<&WidgetId>,
) -> SidebarPanelLayout {
    let layout = mac_sidebar_panel_layout(panel, font, line_height, x, y, w, h);

    if w <= 0.0 || h <= 0.0 {
        return layout;
    }

    if let (Some(bar), Some(tb)) = (&panel.toolbar, layout.toolbar_bounds) {
        let _ = super::toolbar::draw_toolbar(
            ctx,
            font,
            tb.x as f64,
            tb.y as f64,
            tb.width as f64,
            tb.height as f64,
            bar,
            theme,
            hovered_toolbar_id,
            pressed_toolbar_id,
        );
    }

    layout
}
