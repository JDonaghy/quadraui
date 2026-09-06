//! `Panel` primitive: a container with optional chrome (title bar,
//! action buttons) wrapping an app-drawn content region. Used for
//! maximizable terminal panels, sidebar subsections, editor group
//! frames — anywhere the content is app-specific but the frame is
//! consistent.
//!
//! The panel primitive **does not hold its content** — it's pure
//! chrome. Apps draw their TreeView / Terminal / Form / whatever into
//! the `content_bounds` rectangle the layout returns.
//!
//! # Backend contract
//!
//! **Declarative chrome.** Render the title bar (if any) + action
//! buttons + border; leave `content_bounds` to the app. Clicks resolve
//! via [`PanelLayout::hit_test`] / [`PanelHit`]: action buttons hit
//! `PanelHit::Action`; the title bar hits `PanelHit::TitleBar` (apps
//! may use this for drag-to-move or focus); content bounds hit
//! `PanelHit::Content` so the app can route further.

use crate::event::Rect;
use crate::types::{Color, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a panel's chrome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Panel {
    pub id: WidgetId,
    /// Title text shown in the chrome title bar. `None` = no title bar.
    #[serde(default)]
    pub title: Option<StyledText>,
    /// Right-aligned action buttons on the title bar (close, maximize,
    /// pin, etc.). Empty = no action buttons.
    #[serde(default)]
    pub actions: Vec<PanelAction>,
    /// Optional accent colour for the title bar background (used to
    /// distinguish focused vs. unfocused panels).
    #[serde(default)]
    pub accent: Option<Color>,
    /// When true, the title bar has a "collapsed" visual and the
    /// content area is skipped in layout (height collapses to just
    /// the title bar).
    #[serde(default)]
    pub collapsed: bool,
}

/// An action button on a panel's title bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelAction {
    pub id: WidgetId,
    /// Glyph to render on the button (e.g. "×", "□", "⚙").
    pub icon: String,
    /// Hover tooltip.
    #[serde(default)]
    pub tooltip: String,
    /// True = render as the "active" toggle state (e.g. pinned).
    #[serde(default)]
    pub is_active: bool,
}

// ── D6 Layout API ───────────────────────────────────────────────────────────

/// Panel chrome measurements.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelMeasure {
    /// Height of the title bar (0 if `panel.title.is_none()`).
    pub title_bar_height: f32,
    /// Width reserved for each action button on the title bar.
    pub action_button_width: f32,
    /// Border/inset around the content area (0 = content fills edge-to-edge).
    pub content_padding: f32,
}

impl PanelMeasure {
    pub fn new(title_bar_height: f32) -> Self {
        Self {
            title_bar_height,
            action_button_width: 24.0,
            content_padding: 0.0,
        }
    }
}

/// Resolved position of one title-bar action button.
#[derive(Debug, Clone, PartialEq)]
pub struct VisiblePanelAction {
    pub action_idx: usize,
    pub id: WidgetId,
    pub bounds: Rect,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelHit {
    /// Click landed on a title-bar action button.
    Action(WidgetId),
    /// Click landed on the title bar body.
    TitleBar(WidgetId),
    /// Click landed in the content region.
    Content(WidgetId),
    /// Click landed outside the panel.
    Outside,
}

/// Fully-resolved panel layout.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelLayout {
    /// Full panel bounds.
    pub bounds: Rect,
    /// Title bar bounds (if present).
    pub title_bar_bounds: Option<Rect>,
    /// Content region bounds. Apps draw their actual content here.
    /// Width/height are `0` when `collapsed = true`.
    pub content_bounds: Rect,
    pub visible_actions: Vec<VisiblePanelAction>,
    pub hit_regions: Vec<(Rect, PanelHit)>,
}

impl PanelLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> PanelHit {
        let inside = x >= self.bounds.x
            && x < self.bounds.x + self.bounds.width
            && y >= self.bounds.y
            && y < self.bounds.y + self.bounds.height;
        if !inside {
            return PanelHit::Outside;
        }
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        PanelHit::Outside
    }
}

impl Panel {
    /// Compute chrome + content layout.
    ///
    /// # Arguments
    ///
    /// - `bounds` — full panel region inside the parent.
    /// - `measure` — chrome dimensions (title bar height, button
    ///   widths, padding).
    pub fn layout(&self, bounds: Rect, measure: PanelMeasure) -> PanelLayout {
        let mut visible_actions: Vec<VisiblePanelAction> = Vec::new();
        let mut hit_regions: Vec<(Rect, PanelHit)> = Vec::new();

        // Title bar (if any).
        let title_bar_bounds = if self.title.is_some() && measure.title_bar_height > 0.0 {
            let tb = Rect::new(bounds.x, bounds.y, bounds.width, measure.title_bar_height);
            Some(tb)
        } else {
            None
        };

        // Action buttons (right-aligned in title bar).
        let mut action_area_right = bounds.x + bounds.width;
        if let Some(tb) = title_bar_bounds {
            for (i, action) in self.actions.iter().enumerate() {
                let ax = action_area_right - measure.action_button_width;
                if ax < tb.x {
                    break;
                }
                let ab = Rect::new(ax, tb.y, measure.action_button_width, tb.height);
                visible_actions.push(VisiblePanelAction {
                    action_idx: i,
                    id: action.id.clone(),
                    bounds: ab,
                });
                hit_regions.push((ab, PanelHit::Action(action.id.clone())));
                action_area_right = ax;
            }
            // Title-bar body (left of action buttons).
            let tb_body = Rect::new(tb.x, tb.y, action_area_right - tb.x, tb.height);
            hit_regions.push((tb_body, PanelHit::TitleBar(self.id.clone())));
        }

        // Content region.
        let content_bounds = if self.collapsed {
            // Collapsed: content region has zero height.
            let y = title_bar_bounds.map(|b| b.y + b.height).unwrap_or(bounds.y);
            Rect::new(bounds.x + measure.content_padding, y, 0.0, 0.0)
        } else {
            let content_y = title_bar_bounds
                .map(|b| b.y + b.height + measure.content_padding)
                .unwrap_or(bounds.y + measure.content_padding);
            let content_h =
                (bounds.y + bounds.height - content_y - measure.content_padding).max(0.0);
            let content_w = (bounds.width - measure.content_padding * 2.0).max(0.0);
            Rect::new(
                bounds.x + measure.content_padding,
                content_y,
                content_w,
                content_h,
            )
        };

        if content_bounds.width > 0.0 && content_bounds.height > 0.0 {
            hit_regions.push((content_bounds, PanelHit::Content(self.id.clone())));
        }

        PanelLayout {
            bounds,
            title_bar_bounds,
            content_bounds,
            visible_actions,
            hit_regions,
        }
    }
}

// ── NativeSurface paint (#859, Phase 2d slice 2/9 of the NativeSurface
// milestone) ─────────────────────────────────────────────────────────────
//
// Before this, `gtk::draw_panel` (Cairo/Pango), `macos::panel::draw_panel`
// (Core Graphics/Core Text) and `win::panel::draw_panel` (Direct2D/
// DirectWrite) each independently painted the same title-bar +
// action-button chrome with their own drawing API (quadraui#785 child
// #811, `docs/SMELL_AUDIT_2026-07.md` §5). `paint` below is the one
// shared implementation, written against
// [`crate::native_surface::NativeSurface`] (#807, Phase 1) instead of any
// one backend's drawing API.
//
// The three deleted copies were near-identical: title bar filled with
// `panel.accent.unwrap_or(theme.separator)`, title text drawn at
// `(tb.x + 4, tb.y)`, action buttons filled with `theme.accent_bg` when
// `is_active` else the title bar's own colour, and each glyph centred
// horizontally in its button and top-aligned at `va.bounds.y`. One real
// (if narrow) divergence was found and is **not** silently resolved here,
// per this issue's "report it" instruction:
//
// - `gtk::draw_panel` and `macos::panel::draw_panel` centred an action
//   glyph with `(va.bounds.width - text_w) / 2.0`, unclamped — if a
//   glyph were ever wider than its 24px button, the computed x would go
//   *negative*, drawing left of the button's own left edge.
// - `win::panel::draw_panel` computed the identical centring offset but
//   wrapped it in `.max(0.0)`, clamping the glyph to the button's left
//   edge instead of overhanging it.
//
// This is unreachable in practice (every caller passes a single-glyph
// icon inside a 24px button, per `PanelMeasure::action_button_width`'s
// doc), which is presumably why it survived three independent
// implementations without a bug report. `paint` below follows the 2-of-3
// majority (GTK, macOS) and does **not** clamp — reported here rather
// than picked silently, so a future issue can decide whether Win's clamp
// should instead become the shared behaviour.
//
// A second divergence, this one a straightforward bug fix rather than a
// judgment call: `gtk::draw_panel` filled the title bar and action
// buttons via `gtk::set_source` (`cr.set_source_rgb`), which drops
// `Color::a` — the exact GTK-only alpha-dropping bug quadraui#811 (slice
// 1/9, `scrollbar`) found and fixed at the source, in
// `GtkBackend::surface_fill_rect` itself. `macos::panel::draw_panel` and
// `win::panel::draw_panel` already passed `Color::a` straight through to
// `CGContextSetRGBFillColor` / `D2D1_COLOR_F`. So a translucent
// `panel.accent` used to render opaque on GTK only; routing through
// `surface_fill_rect` (already alpha-correct since #811) makes GTK match
// macOS/Windows instead of reintroducing the bug, with no extra fix
// needed here. Themes ship only opaque colours today, so this has no
// visible effect until a caller sets a translucent `panel.accent`.
//
// `#[allow(dead_code)]`: see `primitives::form`'s identical note (#808)
// — only *called* once a real pixel backend is compiled in, exercised by
// each backend's own `Backend::draw_panel` call site plus this module's
// own `RecordingSurface` tests on every leg that enables one of the three
// cfg'd features.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{Panel, PanelLayout};
    use crate::event::Rect;
    use crate::native_surface::NativeSurface;
    use crate::theme::Theme;

    fn plain_text(t: &crate::types::StyledText) -> String {
        t.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// Paint a [`Panel`]'s chrome (title bar + action buttons) onto
    /// `surface`. `layout` must be the same [`PanelLayout`] the caller
    /// uses for hit-testing (typically `Backend::panel_layout`'s return
    /// value, or the value this fn's own caller — `Backend::draw_panel`
    /// — returns) so paint and hit-test can never disagree. Content is
    /// NOT painted — every backend leaves `layout.content_bounds` to the
    /// app, same contract as before this migration.
    pub(crate) fn paint(
        panel: &Panel,
        layout: &PanelLayout,
        surface: &mut dyn NativeSurface,
        theme: &Theme,
    ) {
        let Some(tb) = layout.title_bar_bounds else {
            return;
        };

        let title_bg = panel.accent.unwrap_or(theme.separator);
        surface.surface_fill_rect(tb, title_bg);

        if let Some(ref title) = panel.title {
            let text = plain_text(title);
            let (tw, th) = surface.surface_measure_text(&text);
            surface.surface_draw_text_run(
                Rect::new(tb.x + 4.0, tb.y, tw, th),
                &text,
                theme.foreground,
            );
        }

        for va in &layout.visible_actions {
            let Some(action) = panel.actions.get(va.action_idx) else {
                continue;
            };
            let action_bg = if action.is_active {
                theme.accent_bg
            } else {
                title_bg
            };
            surface.surface_fill_rect(va.bounds, action_bg);

            let (gw, gh) = surface.surface_measure_text(&action.icon);
            // Unclamped — see this module's doc for the named win vs.
            // gtk/macos divergence.
            let glyph_x = va.bounds.x + (va.bounds.width - gw) / 2.0;
            surface.surface_draw_text_run(
                Rect::new(glyph_x, va.bounds.y, gw, gh),
                &action.icon,
                theme.foreground,
            );
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::panel::{PanelAction, PanelMeasure};
        use crate::types::{Color, StyledText, WidgetId};
        use crate::Image;

        /// Records every surface verb this primitive's paint uses —
        /// mirrors `primitives::scrollbar`'s `RecordingSurface` test
        /// double, so this test runs on any host without Cairo/Core
        /// Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            text_runs: Vec<(Rect, String, Color)>,
        }

        impl NativeSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(200.0, 100.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 16.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(
                &mut self,
                _from: crate::Point,
                _to: crate::Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn sample_panel() -> Panel {
            Panel {
                id: WidgetId::new("p"),
                title: Some(StyledText::plain("Terminal")),
                actions: vec![
                    PanelAction {
                        id: WidgetId::new("p:close"),
                        icon: "x".into(),
                        tooltip: String::new(),
                        is_active: false,
                    },
                    PanelAction {
                        id: WidgetId::new("p:pin"),
                        icon: "o".into(),
                        tooltip: String::new(),
                        is_active: true,
                    },
                ],
                accent: None,
                collapsed: false,
            }
        }

        #[test]
        fn title_bar_paints_accent_when_set_else_separator() {
            let mut panel = sample_panel();
            let bounds = Rect::new(0.0, 0.0, 200.0, 100.0);
            let layout = panel.layout(bounds, PanelMeasure::new(20.0));
            let theme = Theme::default();

            let mut surface = RecordingSurface::default();
            paint(&panel, &layout, &mut surface, &theme);
            assert_eq!(surface.fills[0].1, theme.separator);

            panel.accent = Some(Color::rgb(10, 20, 30));
            let layout = panel.layout(bounds, PanelMeasure::new(20.0));
            let mut surface = RecordingSurface::default();
            paint(&panel, &layout, &mut surface, &theme);
            assert_eq!(surface.fills[0].1, panel.accent.unwrap());
        }

        #[test]
        fn title_text_drawn_at_title_bar_top_left_inset() {
            let panel = sample_panel();
            let bounds = Rect::new(0.0, 0.0, 200.0, 100.0);
            let layout = panel.layout(bounds, PanelMeasure::new(20.0));
            let theme = Theme::default();

            let mut surface = RecordingSurface::default();
            paint(&panel, &layout, &mut surface, &theme);

            let tb = layout.title_bar_bounds.unwrap();
            let (rect, text, color) = &surface.text_runs[0];
            assert_eq!(text, "Terminal");
            assert_eq!(rect.x, tb.x + 4.0);
            assert_eq!(rect.y, tb.y);
            assert_eq!(*color, theme.foreground);
        }

        #[test]
        fn active_action_paints_accent_bg_inactive_paints_title_bg() {
            let panel = sample_panel();
            let bounds = Rect::new(0.0, 0.0, 200.0, 100.0);
            let layout = panel.layout(bounds, PanelMeasure::new(20.0));
            let theme = Theme::default();

            let mut surface = RecordingSurface::default();
            paint(&panel, &layout, &mut surface, &theme);

            // fills[0] = title bar, then one fill + one text run per
            // visible action, in `visible_actions` order.
            let close_fill = surface.fills[1].1;
            let pin_fill = surface.fills[2].1;
            assert_eq!(
                close_fill, theme.separator,
                "inactive action keeps title bg"
            );
            assert_eq!(pin_fill, theme.accent_bg, "active action gets accent_bg");
        }

        #[test]
        fn action_glyph_centred_horizontally_in_its_button() {
            let panel = sample_panel();
            let bounds = Rect::new(0.0, 0.0, 200.0, 100.0);
            let layout = panel.layout(bounds, PanelMeasure::new(20.0));
            let theme = Theme::default();

            let mut surface = RecordingSurface::default();
            paint(&panel, &layout, &mut surface, &theme);

            let va = &layout.visible_actions[0];
            let (rect, _, _) = surface
                .text_runs
                .iter()
                .find(|(_, t, _)| t == "x")
                .expect("close glyph drawn");
            let expected_x = va.bounds.x + (va.bounds.width - rect.width) / 2.0;
            assert!((rect.x - expected_x).abs() < 0.01);
        }

        #[test]
        fn no_title_paints_nothing() {
            let panel = Panel {
                id: WidgetId::new("p"),
                title: None,
                actions: vec![],
                accent: None,
                collapsed: false,
            };
            let bounds = Rect::new(0.0, 0.0, 200.0, 100.0);
            let layout = panel.layout(bounds, PanelMeasure::new(20.0));
            let theme = Theme::default();

            let mut surface = RecordingSurface::default();
            paint(&panel, &layout, &mut surface, &theme);
            assert!(surface.fills.is_empty());
            assert!(surface.text_runs.is_empty());
        }

        #[test]
        fn collapsed_panel_still_paints_title_bar_chrome() {
            let mut panel = sample_panel();
            panel.collapsed = true;
            let bounds = Rect::new(0.0, 0.0, 200.0, 100.0);
            let layout = panel.layout(bounds, PanelMeasure::new(20.0));
            let theme = Theme::default();

            let mut surface = RecordingSurface::default();
            paint(&panel, &layout, &mut surface, &theme);
            assert!(
                !surface.fills.is_empty(),
                "title bar chrome still paints when collapsed"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Panel primitive tests ─────────────────────────────────────────

    fn mk_panel_action(id: &str, icon: char) -> PanelAction {
        PanelAction {
            id: WidgetId::new(id),
            icon: icon.to_string(),
            tooltip: String::new(),
            is_active: false,
        }
    }

    #[test]
    fn panel_layout_with_title_and_actions() {
        let p = Panel {
            id: WidgetId::new("terminal"),
            title: Some(StyledText::plain("Terminal")),
            actions: vec![
                mk_panel_action("term:split", '+'),
                mk_panel_action("term:max", '□'),
                mk_panel_action("term:close", '×'),
            ],
            accent: None,
            collapsed: false,
        };
        let bounds = Rect::new(0.0, 0.0, 400.0, 200.0);
        let layout = p.layout(bounds, PanelMeasure::new(24.0));
        assert!(layout.title_bar_bounds.is_some());
        let tb = layout.title_bar_bounds.unwrap();
        assert_eq!(tb.height, 24.0);
        // Actions right-aligned: close (last in actions) at rightmost.
        assert_eq!(layout.visible_actions.len(), 3);
        assert_eq!(layout.visible_actions[0].id.as_str(), "term:split");
        // Rightmost action is first in iteration (right-to-left placement).
        assert_eq!(
            layout.visible_actions[0].bounds.x + layout.visible_actions[0].bounds.width,
            400.0
        );
        // Content region below title bar.
        assert_eq!(layout.content_bounds.y, 24.0);
        assert_eq!(layout.content_bounds.height, 200.0 - 24.0);
    }

    #[test]
    fn panel_layout_no_title() {
        let p = Panel {
            id: WidgetId::new("p"),
            title: None,
            actions: vec![],
            accent: None,
            collapsed: false,
        };
        let bounds = Rect::new(10.0, 20.0, 300.0, 100.0);
        let layout = p.layout(bounds, PanelMeasure::new(24.0));
        assert!(layout.title_bar_bounds.is_none());
        // Content fills the full panel.
        assert_eq!(layout.content_bounds.y, 20.0);
        assert_eq!(layout.content_bounds.height, 100.0);
    }

    #[test]
    fn panel_layout_collapsed() {
        let p = Panel {
            id: WidgetId::new("p"),
            title: Some(StyledText::plain("Collapsed")),
            actions: vec![],
            accent: None,
            collapsed: true,
        };
        let bounds = Rect::new(0.0, 0.0, 200.0, 150.0);
        let layout = p.layout(bounds, PanelMeasure::new(20.0));
        // Title bar still rendered; content region has zero size.
        assert!(layout.title_bar_bounds.is_some());
        assert_eq!(layout.content_bounds.width, 0.0);
        assert_eq!(layout.content_bounds.height, 0.0);
    }

    #[test]
    fn panel_layout_hit_test_dispatches_correctly() {
        let p = Panel {
            id: WidgetId::new("p"),
            title: Some(StyledText::plain("T")),
            actions: vec![mk_panel_action("close", '×')],
            accent: None,
            collapsed: false,
        };
        let bounds = Rect::new(0.0, 0.0, 200.0, 100.0);
        let layout = p.layout(bounds, PanelMeasure::new(20.0));
        // Click on the close button (rightmost in title bar).
        let close = &layout.visible_actions[0];
        let cx = close.bounds.x + close.bounds.width / 2.0;
        let cy = close.bounds.y + close.bounds.height / 2.0;
        match layout.hit_test(cx, cy) {
            PanelHit::Action(id) => assert_eq!(id.as_str(), "close"),
            _ => panic!("expected Action(close)"),
        }
        // Click on title bar body.
        match layout.hit_test(20.0, 10.0) {
            PanelHit::TitleBar(id) => assert_eq!(id.as_str(), "p"),
            _ => panic!("expected TitleBar"),
        }
        // Click on content region.
        match layout.hit_test(100.0, 50.0) {
            PanelHit::Content(id) => assert_eq!(id.as_str(), "p"),
            _ => panic!("expected Content"),
        }
        // Click outside.
        assert_eq!(layout.hit_test(500.0, 500.0), PanelHit::Outside);
    }
}
