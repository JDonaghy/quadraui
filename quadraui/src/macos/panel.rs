//! macOS rasteriser for [`crate::Panel`].
//!
//! Painting goes through `MacBackend::draw_panel`, which maps the title
//! bar onto a [`crate::SidebarPanel`] + [`crate::Toolbar`] header (via
//! [`Panel::header_toolbar`][crate::primitives::panel::Panel::header_toolbar])
//! and paints it with
//! [`crate::primitives::sidebar_panel::native_surface_paint::paint`].
//! This module provides [`mac_panel_layout`] for pure layout calculations,
//! used by `MacBackend::panel_layout` (and as `draw_panel`'s own hit-test
//! return value) for hit-test queries.

#[cfg(test)]
use core_text::font::CTFont;

use crate::primitives::layout_metrics::pixel_panel_layout;
// `Panel` is `#[deprecated]` (use `SidebarPanel` + `Toolbar` instead); this
// whole module is `Panel`'s own macOS rasteriser plumbing.
#[allow(deprecated)]
use crate::primitives::panel::{Panel, PanelLayout};
#[cfg(test)]
use crate::theme::Theme;

/// Compute the macOS pixel-unit layout for a [`Panel`] without painting.
/// Shares its action-button width with `gtk_panel_layout` /
/// `win_panel_layout` via [`pixel_panel_layout`].
#[allow(deprecated)] // `Panel` is `#[deprecated]`; this fn is its own plumbing.
pub fn mac_panel_layout(
    panel: &Panel,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_height: f64,
) -> PanelLayout {
    pixel_panel_layout(
        panel,
        x as f32,
        y as f32,
        w as f32,
        h as f32,
        line_height as f32,
    )
}

#[cfg(test)]
#[allow(deprecated)] // `Panel`/`PanelAction` are `#[deprecated]`; these tests cover their macOS rasteriser directly.
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::panel::{PanelAction, PanelHit};
    use crate::types::{Color, StyledText, WidgetId};
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
        assert!(
            (first.bounds.x
                - (W as f32 - crate::primitives::layout_metrics::pixel::PANEL_ACTION_BUTTON))
                .abs()
                < 0.5
        );
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
    fn hit_test_resolves_action_vs_title_vs_content_at_nonzero_origin() {
        // Layout coordinates must be consistent across backends: verify that
        // `mac_panel_layout` returns absolute frame coordinates and action/title/
        // content hits resolve correctly at non-zero origins.
        let panel = sample_panel();
        let layout = mac_panel_layout(&panel, 7.0, 13.0, W as f64, H as f64, 16.0);

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

    /// Paints with a known theme (white bg/header_bg/separator, black
    /// foreground) and reads back actual pixels, so "ink" means "not
    /// white" — the same idiom `GtkBackend`'s sibling test uses.
    fn paint_via_backend_with_theme(panel: &Panel, theme: Theme) -> (BitmapSurface, PanelLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.set_current_theme(theme);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_panel(QRect::new(0.0, 0.0, W as f32, H as f32), panel);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    /// `MacBackend::draw_panel` paints its title bar through the mapped
    /// `SidebarPanel` + `Toolbar` header (`Panel::header_toolbar`) rather
    /// than `panel::native_surface_paint::paint`'s own painter, while
    /// `draw_panel` still returns `mac_panel_layout(...)` for hit-testing.
    /// Pins the two paint↔layout agreements that migration depends on:
    /// the title text paints inset from the title bar's left edge (not
    /// flush against it), and each action glyph paints inside the exact
    /// `visible_actions[i].bounds` rect the returned layout reports.
    #[test]
    fn draw_panel_paints_title_inset_and_actions_in_bounds() {
        let white = Color::rgb(255, 255, 255);
        let theme = Theme {
            background: white,
            header_bg: white,
            separator: white,
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };
        let panel = sample_panel();
        let (surface, layout) = paint_via_backend_with_theme(&panel, theme);

        let is_ink = |x: u32, y: u32| {
            let (r, g, b, _) = surface.pixel(x, y);
            !(r == 255 && g == 255 && b == 255)
        };

        let tb = layout.title_bar_bounds.expect("title bar present");
        assert_eq!(
            layout.visible_actions.len(),
            2,
            "both actions should fit in a {W}px-wide title bar"
        );
        // Bound the ink scan to the title body, left of the leftmost
        // action button — scanning the whole row would let the first
        // action glyph (not the title) be the "first ink" found, which
        // would pass even if the title painted nothing at all.
        let title_body_right = layout
            .visible_actions
            .iter()
            .map(|va| va.bounds.x)
            .fold(f32::INFINITY, f32::min);
        let row_top = tb.y.round() as u32;
        let row_bottom = (tb.y + tb.height).round() as u32;
        let first_ink_col = (tb.x.round() as u32..title_body_right.round() as u32)
            .find(|&x| (row_top..row_bottom).any(|y| is_ink(x, y)))
            .expect("title text must paint some ink in the title bar row");
        assert!(
            first_ink_col as f32 > tb.x,
            "title text must be inset from the title bar's left edge: \
             first_ink_col={first_ink_col}, tb.x={}",
            tb.x
        );
        for va in &layout.visible_actions {
            let x0 = va.bounds.x.round() as u32;
            let x1 = (va.bounds.x + va.bounds.width).round() as u32;
            let y0 = va.bounds.y.round() as u32;
            let y1 = (va.bounds.y + va.bounds.height).round() as u32;
            let has_ink = (x0..x1).any(|x| (y0..y1).any(|y| is_ink(x, y)));
            assert!(
                has_ink,
                "action {:?} glyph must paint ink inside its own bounds {:?}",
                va.id, va.bounds,
            );
        }
    }

    /// Pins the title's painted colour to `theme.foreground`, not the
    /// `sidebar_panel::ToolbarButton::Label` default of `theme.muted_fg`
    /// — `MacBackend::draw_panel` sets the mapped label's `fg` explicitly
    /// so the title keeps the contrast `panel::native_surface_paint::
    /// paint` drew it with.
    #[test]
    fn draw_panel_title_paints_theme_foreground() {
        let background = Color::rgb(255, 255, 255);
        let foreground = Color::rgb(10, 200, 10);
        let muted_fg = Color::rgb(120, 122, 135);
        let theme = Theme {
            background,
            header_bg: background,
            separator: background,
            foreground,
            muted_fg,
            ..Theme::default()
        };
        let panel = sample_panel();
        let (surface, layout) = paint_via_backend_with_theme(&panel, theme);

        let tb = layout.title_bar_bounds.expect("title bar present");
        let title_body_right = layout
            .visible_actions
            .iter()
            .map(|va| va.bounds.x)
            .fold(f32::INFINITY, f32::min);
        let row_top = tb.y.round() as u32;
        let row_bottom = (tb.y + tb.height).round() as u32;

        let mut found_fg = false;
        let mut found_muted = false;
        for x in (tb.x.round() as u32)..(title_body_right.round() as u32) {
            for y in row_top..row_bottom {
                let (r, g, b, _) = surface.pixel(x, y);
                if (r, g, b) == (foreground.r, foreground.g, foreground.b) {
                    found_fg = true;
                }
                if (r, g, b) == (muted_fg.r, muted_fg.g, muted_fg.b) {
                    found_muted = true;
                }
            }
        }
        assert!(
            found_fg,
            "title text must paint in theme.foreground somewhere in the title body"
        );
        assert!(
            !found_muted,
            "title text must not fall back to theme.muted_fg"
        );
    }

    /// Each action button's hit-test rect (`mac_panel_layout`'s fixed
    /// `PANEL_ACTION_BUTTON`-wide slot) must stay the slot the matching
    /// glyph actually paints into, even when the actions' icon strings
    /// measure to different widths — `draw_panel` paints action glyphs
    /// directly against `layout.visible_actions[i].bounds` rather than
    /// through `sidebar_panel::native_surface_paint::paint`'s own
    /// label-width-based measure, so the two can never drift apart the
    /// way they would if action paint were delegated too.
    #[test]
    fn draw_panel_action_glyphs_stay_in_fixed_width_bounds_with_differing_widths() {
        let white = Color::rgb(255, 255, 255);
        let theme = Theme {
            background: white,
            header_bg: white,
            separator: white,
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };
        let mut panel = sample_panel();
        panel.actions = vec![
            PanelAction {
                id: WidgetId::new("a0"),
                icon: "×".into(),
                tooltip: String::new(),
                is_active: false,
            },
            PanelAction {
                id: WidgetId::new("a1"),
                icon: "settings".into(),
                tooltip: String::new(),
                is_active: false,
            },
            PanelAction {
                id: WidgetId::new("a2"),
                icon: "□".into(),
                tooltip: String::new(),
                is_active: false,
            },
            PanelAction {
                id: WidgetId::new("a3"),
                icon: "w".into(),
                tooltip: String::new(),
                is_active: false,
            },
        ];
        let (surface, layout) = paint_via_backend_with_theme(&panel, theme);

        let is_ink = |x: u32, y: u32| {
            let (r, g, b, _) = surface.pixel(x, y);
            !(r == 255 && g == 255 && b == 255)
        };

        assert_eq!(layout.visible_actions.len(), 4);
        for va in &layout.visible_actions {
            assert_eq!(
                va.bounds.width,
                crate::primitives::layout_metrics::pixel::PANEL_ACTION_BUTTON,
                "action {:?} must keep the fixed PANEL_ACTION_BUTTON width",
                va.id,
            );
            let x0 = va.bounds.x.round() as u32;
            let x1 = (va.bounds.x + va.bounds.width).round() as u32;
            let y0 = va.bounds.y.round() as u32;
            let y1 = (va.bounds.y + va.bounds.height).round() as u32;
            let has_ink = (x0..x1).any(|x| (y0..y1).any(|y| is_ink(x, y)));
            assert!(
                has_ink,
                "action {:?} glyph must paint ink inside its own hit-test bounds {:?}",
                va.id, va.bounds,
            );
        }
    }

    /// `sidebar_panel::native_surface_paint`'s own active-state highlight
    /// is a 2px-inset `theme.selected_bg` pill, not a full-bounds fill —
    /// `MacBackend::draw_panel` re-fills a `PanelAction::is_active`
    /// button's *entire* bounds with `theme.accent_bg` afterward (there's
    /// no interaction state to drive hover/press from `draw_panel`'s
    /// non-interactive signature, so `is_active` is the only state left
    /// to carry), keeping `panel::native_surface_paint::paint`'s old
    /// full-rect-fill contract. Checked at the button's own corner pixel
    /// — inside an inset pill's margin but inside a full-rect fill — so
    /// this fails if the override regresses to the inset-only highlight.
    #[test]
    fn draw_panel_is_active_action_fills_full_bounds_with_accent_bg() {
        let accent_bg = Color::rgb(10, 20, 200);
        let white = Color::rgb(255, 255, 255);
        let theme = Theme {
            background: white,
            header_bg: white,
            separator: white,
            foreground: Color::rgb(0, 0, 0),
            accent_bg,
            ..Theme::default()
        };
        let mut panel = sample_panel();
        panel.actions = vec![PanelAction {
            id: WidgetId::new("pin"),
            icon: "*".into(),
            tooltip: String::new(),
            is_active: true,
        }];
        let (surface, layout) = paint_via_backend_with_theme(&panel, theme);

        let va = &layout.visible_actions[0];
        let corner_x = va.bounds.x.round() as u32;
        let corner_y = va.bounds.y.round() as u32;
        let (r, g, b, _) = surface.pixel(corner_x, corner_y);
        assert_eq!(
            (r, g, b),
            (accent_bg.r, accent_bg.g, accent_bg.b),
            "an is_active action's full bounds (down to its own corner) must \
             fill with theme.accent_bg, not an inset pill",
        );
    }
}
