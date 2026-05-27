//! GTK rasteriser for [`crate::primitives::sidebar_panel::SidebarPanel`].
//!
//! Paints the optional toolbar header by delegating to
//! [`super::draw_toolbar`]. The content rect is **not** painted —
//! returned in `SidebarPanelLayout.content_bounds` for the host to
//! draw into (mirrors the existing `draw_panel` contract).

use gtk4::cairo::Context;
use gtk4::pango;

use crate::primitives::sidebar_panel::{SidebarPanel, SidebarPanelLayout, SidebarPanelMeasure};
use crate::primitives::toolbar::ToolbarItemMeasure;
use crate::theme::Theme;
use crate::types::WidgetId;

/// Compute the GTK pixel-unit layout for a `SidebarPanel`. Uses Pango
/// for accurate text measurement when `pango_layout` is provided; falls
/// back to a `char_width`-based estimate otherwise (matches the
/// `gtk_toolbar_layout` fallback convention).
#[allow(clippy::too_many_arguments)]
pub fn gtk_sidebar_panel_layout(
    panel: &SidebarPanel,
    pango_layout: Option<&pango::Layout>,
    char_width: f64,
    line_height: f64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> SidebarPanelLayout {
    let bounds = crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32);
    panel.layout(
        bounds,
        SidebarPanelMeasure::new(line_height as f32, char_width as f32),
        |btn| ToolbarItemMeasure::new(item_width_px(pango_layout, char_width, btn) as f32),
    )
}

fn item_width_px(
    pango_layout: Option<&pango::Layout>,
    char_width: f64,
    btn: &crate::primitives::toolbar::ToolbarButton,
) -> f64 {
    // Mirror gtk::toolbar::measure_item, but inlined here so we don't
    // export that helper just for this caller.
    use crate::primitives::toolbar::ToolbarButton;
    const ACTION_H_PAD: f64 = 8.0;
    const SEPARATOR_PX: f64 = 12.0;
    let text_width = |text: &str| -> f64 {
        if let Some(pl) = pango_layout {
            pl.set_text(text);
            pl.pixel_size().0.max(0) as f64
        } else {
            (text.chars().count() as f64 * char_width).ceil()
        }
    };
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
            text_width(&s) + 2.0 * ACTION_H_PAD
        }
        ToolbarButton::Separator => SEPARATOR_PX,
        ToolbarButton::Label { text, .. } => text_width(text),
    }
}

/// Draw a `SidebarPanel` onto `cr`. Returns the resolved layout for
/// the host to paint content into `content_bounds` and route clicks
/// via `hit_test`.
#[allow(clippy::too_many_arguments)]
pub fn draw_sidebar_panel(
    cr: &Context,
    pango_layout: &pango::Layout,
    line_height: f64,
    char_width: f64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    panel: &SidebarPanel,
    theme: &Theme,
    hovered_toolbar_id: Option<&WidgetId>,
    pressed_toolbar_id: Option<&WidgetId>,
) -> SidebarPanelLayout {
    let layout = gtk_sidebar_panel_layout(
        panel,
        Some(pango_layout),
        char_width,
        line_height,
        x,
        y,
        w,
        h,
    );

    if w <= 0.0 || h <= 0.0 {
        return layout;
    }

    if let (Some(bar), Some(tb_bounds)) = (&panel.toolbar, layout.toolbar_bounds) {
        let _ = super::draw_toolbar(
            cr,
            pango_layout,
            tb_bounds.x as f64,
            tb_bounds.y as f64,
            tb_bounds.width as f64,
            tb_bounds.height as f64,
            bar,
            theme,
            hovered_toolbar_id,
            pressed_toolbar_id,
        );
    }

    layout
}
