//! macOS rasteriser for [`crate::Panel`].
//!
//! Paints panel chrome (title bar + action buttons). Content is the
//! app's responsibility — the rasteriser returns the resolved
//! [`PanelLayout`] so the host paints into `content_bounds` and routes
//! clicks via `hit_test`.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::event::Rect as QRect;
use crate::primitives::panel::{Panel, PanelLayout, PanelMeasure};
use crate::theme::Theme;
use crate::types::Color;

/// 24-pt action-button width, matching GTK.
const ACTION_BUTTON_PX: f32 = 24.0;

/// Compute the macOS pixel-unit layout for a [`Panel`] without painting.
pub fn mac_panel_layout(
    panel: &Panel,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_height: f64,
) -> PanelLayout {
    let bounds = QRect::new(x as f32, y as f32, w as f32, h as f32);
    let measure = PanelMeasure {
        title_bar_height: if panel.title.is_some() {
            line_height as f32
        } else {
            0.0
        },
        action_button_width: ACTION_BUTTON_PX,
        content_padding: 0.0,
    };
    panel.layout(bounds, measure)
}

/// Draw a [`Panel`]'s chrome onto `ctx`. Returns the layout for host
/// click dispatch. Content is NOT painted.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_panel(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    panel: &Panel,
    theme: &Theme,
    line_height: f64,
) -> PanelLayout {
    let layout = mac_panel_layout(panel, x, y, w, h, line_height);

    if let Some(tb) = layout.title_bar_bounds {
        let title_bg = panel.accent.unwrap_or(theme.separator);
        fill_rect(
            ctx,
            tb.x as f64,
            tb.y as f64,
            tb.width as f64,
            tb.height as f64,
            title_bg,
        );

        // Title text.
        if let Some(ref title) = panel.title {
            let text: String = title.spans.iter().map(|s| s.text.as_str()).collect();
            draw_text(
                ctx,
                font,
                &text,
                tb.x as f64 + 4.0,
                tb.y as f64,
                color_to_cg(theme.foreground),
            );
        }

        // Action buttons.
        for va in &layout.visible_actions {
            let action = &panel.actions[va.action_idx];
            let action_bg = if action.is_active {
                theme.accent_bg
            } else {
                title_bg
            };
            fill_rect(
                ctx,
                va.bounds.x as f64,
                va.bounds.y as f64,
                va.bounds.width as f64,
                va.bounds.height as f64,
                action_bg,
            );
            let (gw, _) = measure_text(font, &action.icon);
            draw_text(
                ctx,
                font,
                &action.icon,
                va.bounds.x as f64 + (va.bounds.width as f64 - gw) / 2.0,
                va.bounds.y as f64,
                color_to_cg(theme.foreground),
            );
        }
    }

    layout
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    use core_graphics::geometry::{CGPoint, CGSize};
    CGContextFillRect(ctx, CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h)));
}

extern "C" {
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::Viewport;
    use crate::primitives::panel::{PanelAction, PanelHit};
    use crate::types::{StyledText, WidgetId};
    use crate::Backend;

    const W: u32 = 240;
    const H: u32 = 160;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_panel() -> Panel {
        Panel {
            id: WidgetId::new("panel"),
            title: Some(StyledText::plain("Terminal")),
            actions: vec![
                PanelAction {
                    id: WidgetId::new("panel:close"),
                    icon: "×".into(),
                    tooltip: "Close".into(),
                    is_active: false,
                },
                PanelAction {
                    id: WidgetId::new("panel:max"),
                    icon: "□".into(),
                    tooltip: "Maximize".into(),
                    is_active: false,
                },
            ],
            accent: None,
            collapsed: false,
        }
    }

    fn paint_via_backend(panel: &Panel) -> (BitmapSurface, PanelLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_panel(QRect::new(0.0, 0.0, W as f32, H as f32), panel);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn title_bar_paints_separator_bg_by_default() {
        let panel = sample_panel();
        let (surface, layout) = paint_via_backend(&panel);
        let theme = Theme::default();
        let tb = layout.title_bar_bounds.expect("title bar present");
        // Probe right side of the title bar, left of the action buttons.
        // First action starts at W - 24; sample around W/2.
        let px = (tb.x + tb.width / 2.0) as u32;
        let py = (tb.y + tb.height - 1.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.separator.r, theme.separator.g, theme.separator.b),
        );
    }

    #[test]
    fn content_bounds_excludes_title_bar() {
        let panel = sample_panel();
        let (_surface, layout) = paint_via_backend(&panel);
        let tb = layout.title_bar_bounds.expect("title bar present");
        let cb = layout.content_bounds;
        assert!(
            cb.y >= tb.y + tb.height,
            "content should start below title bar: tb.bottom={}, content.y={}",
            tb.y + tb.height,
            cb.y,
        );
    }

    #[test]
    fn action_buttons_right_aligned() {
        let panel = sample_panel();
        let (_surface, layout) = paint_via_backend(&panel);
        // Two actions, each 24px wide. Right-most at (W-24..W).
        assert_eq!(layout.visible_actions.len(), 2);
        let first = &layout.visible_actions[0];
        // First action_idx=0 painted right-most.
        assert!((first.bounds.x - (W as f32 - ACTION_BUTTON_PX)).abs() < 0.5);
    }

    #[test]
    fn hit_test_resolves_action_vs_title_vs_content() {
        let panel = sample_panel();
        let (_surface, layout) = paint_via_backend(&panel);
        let first_action = &layout.visible_actions[0];
        let hit = layout.hit_test(
            first_action.bounds.x + first_action.bounds.width * 0.5,
            first_action.bounds.y + first_action.bounds.height * 0.5,
        );
        assert!(matches!(hit, PanelHit::Action(_)));

        // Title body (left of actions).
        let tb = layout.title_bar_bounds.unwrap();
        let hit = layout.hit_test(tb.x + 10.0, tb.y + tb.height * 0.5);
        assert!(matches!(hit, PanelHit::TitleBar(_)), "hit was {:?}", hit);

        // Content area.
        let cb = layout.content_bounds;
        let hit = layout.hit_test(cb.x + cb.width * 0.5, cb.y + cb.height * 0.5);
        assert!(matches!(hit, PanelHit::Content(_)));
    }

    #[test]
    fn collapsed_panel_zero_content_height() {
        let mut panel = sample_panel();
        panel.collapsed = true;
        let (_surface, layout) = paint_via_backend(&panel);
        assert_eq!(layout.content_bounds.height, 0.0);
    }
}
