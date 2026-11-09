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
//!
//! # Prefer `SidebarPanel` + `Toolbar` for new chrome
//!
//! [`crate::SidebarPanel`] with a [`crate::Toolbar`] header — a
//! [`crate::ToolbarButton::Label`] title, a [`crate::ToolbarButton::Spacer`],
//! then right-aligned [`crate::ToolbarButton::Action`]s — covers the same
//! title-bar-plus-actions shape without a third action-button type
//! alongside `PanelAction` and `SidebarPanel`'s own toolbar actions.
//! [`Panel::layout`] already delegates its action right-alignment
//! arithmetic to [`crate::Toolbar::layout_with_options`]'s `Spacer`
//! mechanism internally. `Panel` keeps working for existing callers;
//! new code should reach for `SidebarPanel` + `Toolbar` first.

use crate::event::Rect;
use crate::primitives::toolbar::{Toolbar, ToolbarButton, ToolbarItemMeasure};
use crate::types::{Color, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a panel's chrome.
///
/// # Examples
///
/// ```
/// use quadraui::{Panel, PanelHit, PanelMeasure, Rect, StyledText, WidgetId};
///
/// let panel = Panel {
///     id: WidgetId::new("panel:terminal"),
///     title: Some(StyledText::plain("Terminal")),
///     actions: vec![],
///     accent: None,
///     collapsed: false,
/// };
///
/// let bounds = Rect::new(0.0, 0.0, 80.0, 24.0);
/// let layout = panel.layout(bounds, PanelMeasure::new(1.0));
///
/// assert_eq!(
///     layout.hit_test(0.0, 0.0),
///     PanelHit::TitleBar(panel.id.clone())
/// );
/// assert_eq!(layout.hit_test(0.0, 2.0), PanelHit::Content(panel.id));
/// ```
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
    /// Count of actions, taken from the front of `self.actions`, that fit
    /// as a contiguous run against `title_bar`'s trailing edge at
    /// `action_button_width` each. An action that wouldn't fully fit is
    /// dropped entirely (not partially clipped) — shared by
    /// [`Self::layout`] (which hands only the fitting actions to
    /// `Toolbar::layout_with_options`, since that clips a too-wide
    /// trailing group instead of dropping items) and
    /// [`Self::header_toolbar`] (which needs the identical count so its
    /// mapped header never paints an action `Panel::layout` would have
    /// dropped).
    fn fitting_action_count(&self, title_bar: Rect, action_button_width: f32) -> usize {
        let mut fit_count = 0usize;
        let mut probe_right = title_bar.x + title_bar.width;
        for _ in &self.actions {
            let ax = probe_right - action_button_width;
            if ax < title_bar.x {
                break;
            }
            probe_right = ax;
            fit_count += 1;
        }
        fit_count
    }

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

        // Action buttons (right-aligned in title bar). The right-alignment
        // arithmetic itself is delegated to `Toolbar::layout_with_options`'s
        // `Spacer` push-to-trailing-edge mechanism — the same mechanism
        // `SidebarPanel`'s own toolbar header slot uses — rather than Panel
        // hand-rolling a second copy of it.
        let mut action_area_right = bounds.x + bounds.width;
        if let Some(tb) = title_bar_bounds {
            let fit_count = self.fitting_action_count(tb, measure.action_button_width);

            // `Toolbar`'s trailing group packs left-to-right in button order,
            // placing the first button leftmost within the group — the
            // opposite of Panel's own "first action is rightmost" contract
            // — so the fitting actions are fed in reverse. `order[k]` names
            // which original `self.actions` index sits at trailing-button
            // position `k`.
            let order: Vec<usize> = (0..fit_count).rev().collect();
            let toolbar_buttons: Vec<ToolbarButton> = std::iter::once(ToolbarButton::Spacer)
                .chain(order.iter().map(|&idx| {
                    let action = &self.actions[idx];
                    ToolbarButton::Action {
                        spec: None,
                        id: action.id,
                        label: action.icon.clone(),
                        icon: None,
                        key_hint: None,
                        enabled: true,
                        is_active: action.is_active,
                        tooltip: action.tooltip.clone(),
                    }
                }))
                .collect();
            let action_toolbar = Toolbar {
                id: self.id,
                buttons: toolbar_buttons,
                bg: None,
                focused_index: None,
            };
            let toolbar_layout = action_toolbar.layout(tb.x, tb.y, tb.width, tb.height, |_| {
                ToolbarItemMeasure::new(measure.action_button_width)
            });

            // Re-key the toolbar's visible items back onto the original
            // `self.actions` index so `visible_actions`/`hit_regions` keep
            // Panel's own iteration-order contract (index 0 first, each
            // test asserting on `visible_actions[i]` by that order).
            let mut action_bounds: Vec<Option<Rect>> = vec![None; self.actions.len()];
            let mut spacer_bounds: Option<Rect> = None;
            for vis in &toolbar_layout.visible_items {
                if vis.item_idx == 0 {
                    spacer_bounds = Some(vis.bounds);
                    continue;
                }
                action_bounds[order[vis.item_idx - 1]] = Some(vis.bounds);
            }
            for (i, action) in self.actions.iter().enumerate() {
                if let Some(ab) = action_bounds[i] {
                    visible_actions.push(VisiblePanelAction {
                        action_idx: i,
                        id: action.id,
                        bounds: ab,
                    });
                    hit_regions.push((ab, PanelHit::Action(action.id)));
                }
            }
            // The spacer's bounds span exactly the gap between the title
            // bar's leading edge and the first visible action (or the full
            // title bar when no action fit) — the same quantity Panel's own
            // hand-rolled loop tracked as `action_area_right`.
            if let Some(sb) = spacer_bounds {
                action_area_right = sb.x + sb.width;
            }
            // Title-bar body (left of action buttons).
            let tb_body = Rect::new(tb.x, tb.y, action_area_right - tb.x, tb.height);
            hit_regions.push((tb_body, PanelHit::TitleBar(self.id)));
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
            hit_regions.push((content_bounds, PanelHit::Content(self.id)));
        }

        PanelLayout {
            bounds,
            title_bar_bounds,
            content_bounds,
            visible_actions,
            hit_regions,
        }
    }

    /// Map this panel's title bar onto an equivalent [`crate::SidebarPanel`]
    /// header [`Toolbar`]: a title [`ToolbarButton::Label`], a
    /// [`ToolbarButton::Spacer`], then one [`ToolbarButton::Action`] per
    /// fitting [`PanelAction`], carrying its `id`, `icon` (as the
    /// button's `label`), `tooltip` and `is_active`. `self.accent` maps
    /// onto [`Toolbar::bg`].
    ///
    /// Action order matches [`Self::layout`]: the first action is
    /// rightmost (buttons are pushed in reverse, since `Toolbar`'s
    /// trailing group packs left-to-right, so the last button pushed —
    /// `self.actions[0]` — ends up closest to the trailing edge). Only
    /// actions `Self::layout` would keep (the same `fitting_action_count`
    /// contiguous-from-the-front run) are included, so a caller that lays
    /// this header out with a fixed-width [`ToolbarItemMeasure`] for
    /// `Action` buttons gets rects identical to
    /// `self.layout(bounds, measure).visible_actions`, in order — no
    /// extra partially-clipped button for one `Self::layout` would have
    /// dropped entirely.
    ///
    /// The title [`ToolbarButton::Label`] carries a leading space ahead
    /// of the title text — `Toolbar`/`SidebarPanel` paint a `Label` flush
    /// against the toolbar's leading edge, with no inset of its own, so
    /// the leading space reproduces the one-cell/4px inset `Panel`'s own
    /// chrome paints, the same trick `compose::help_layer` already uses
    /// for its own `SidebarPanel` + `Toolbar` title.
    ///
    /// Returns `None` when there's no title bar to paint (`self.title`
    /// is `None`, or `measure.title_bar_height <= 0.0`) — mirrors
    /// [`PanelLayout::title_bar_bounds`]. Layout is unaffected by
    /// `self.collapsed` — the title bar still paints when collapsed, same
    /// as [`Self::layout`].
    pub fn header_toolbar(&self, bounds: Rect, measure: PanelMeasure) -> Option<Toolbar> {
        let title = self.title.as_ref()?;
        if measure.title_bar_height <= 0.0 {
            return None;
        }
        let tb = Rect::new(bounds.x, bounds.y, bounds.width, measure.title_bar_height);
        let fit_count = self.fitting_action_count(tb, measure.action_button_width);
        let order: Vec<usize> = (0..fit_count).rev().collect();

        let mut buttons = vec![
            ToolbarButton::Label {
                text: format!(" {}", plain_text(title)),
                fg: None,
            },
            ToolbarButton::Spacer,
        ];
        buttons.extend(order.iter().map(|&idx| {
            let action = &self.actions[idx];
            ToolbarButton::Action {
                spec: None,
                id: action.id,
                label: action.icon.clone(),
                icon: None,
                key_hint: None,
                enabled: true,
                is_active: action.is_active,
                tooltip: action.tooltip.clone(),
            }
        }));

        Some(Toolbar {
            id: self.id,
            buttons,
            bg: self.accent,
            focused_index: None,
        })
    }
}

/// Flatten a [`StyledText`]'s spans into a plain string — same idiom
/// used throughout the crate wherever a declarative `StyledText` needs
/// feeding into a plain-`String` field (e.g. [`ToolbarButton::Label::text`]).
fn plain_text(t: &StyledText) -> String {
    t.spans.iter().map(|s| s.text.as_str()).collect()
}

// Shared implementation for painting panel title bar and action buttons,
// replacing separate platform-specific implementations that used Cairo/Pango,
// Core Graphics/Core Text, and Direct2D/DirectWrite respectively.
//
// The implementations were near-identical: title bar filled with
// `panel.accent.unwrap_or(theme.separator)`, title text drawn at
// `(tb.x + 4, tb.y)`, action buttons filled with `theme.accent_bg` when
// `is_active` else the title bar's own colour, and each glyph centred
// horizontally in its button and top-aligned at `va.bounds.y`.
//
// One divergence exists: GTK and macOS centre an action glyph with
// `(va.bounds.width - text_w) / 2.0` unclamped (allowing overhang), while
// Windows clamps this to `.max(0.0)`. This implementation follows GTK/macOS
// (the 2-of-3 majority) without clamping.
//
// Another fix: this routes through `surface_fill_rect` (alpha-correct since
// #811), whereas the old GTK implementation dropped `Color::a` via
// `gtk::set_source`, making translucent `panel.accent` render opaque on GTK
// only. macOS and Windows already preserved alpha. Themes ship only opaque
// colours today, so this has no visible effect until a caller sets a
// translucent `panel.accent`.
//
// `#[allow(dead_code)]`: only called once a real pixel backend is compiled
// in, exercised by each backend's `Backend::draw_panel` call site plus
// `RecordingSurface` tests.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{Panel, PanelLayout};
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
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
        surface: &mut dyn PaintSurface,
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
            // `surface_draw_icon_glyph`, not `surface_draw_text_run` —
            // `PanelAction::icon` is a plain app-chosen string (no
            // `Icon::glyph`/`fallback` split to pick between), so an app
            // that wants VS Code-style chrome (a codicon codepoint,
            // matching the maximise/close affordances this crate's own
            // chrome paints as codicon glyphs elsewhere) needs this
            // glyph to resolve through the same icon-font fallback path
            // those use, not whatever plain chrome font is live.
            surface.surface_draw_icon_glyph(
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

        impl PaintSurface for RecordingSurface {
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
            /// Test-only recorder — `paint` never calls this verb
            /// (see this module's own doc for why no primitive here has been
            /// migrated onto it yet); this exists only so `RecordingSurface`
            /// satisfies the trait. Records into the same `fills` list as
            /// `surface_fill_rect` (radius dropped) — no test asserts on it
            /// today.
            fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
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

    // ── `header_toolbar` mapping ───────────────────────────────────────

    use crate::primitives::toolbar::ToolbarLayoutOptions;
    use std::collections::HashMap;

    /// Build a panel with `action_count` actions (ids "a0".."aN") and
    /// assert `panel.header_toolbar(bounds, measure)`, laid out with a
    /// fixed-width measure for `Action` buttons, produces action rects
    /// identical to `panel.layout(bounds, measure).visible_actions` —
    /// same count, same bounds per id.
    fn assert_header_matches_layout(bounds: Rect, measure: PanelMeasure, action_count: usize) {
        let actions: Vec<PanelAction> = (0..action_count)
            .map(|i| mk_panel_action(&format!("a{i}"), 'x'))
            .collect();
        let panel = Panel {
            id: WidgetId::new("p"),
            title: Some(StyledText::plain("Title")),
            actions,
            accent: Some(Color::rgb(10, 20, 30)),
            collapsed: false,
        };

        let panel_layout = panel.layout(bounds, measure);
        let header = panel
            .header_toolbar(bounds, measure)
            .expect("title present, title_bar_height > 0");

        // Accent maps onto `Toolbar.bg`.
        assert_eq!(header.bg, panel.accent);

        // Title `Label` keeps the leading-space inset.
        match &header.buttons[0] {
            ToolbarButton::Label { text, .. } => assert_eq!(text, " Title"),
            other => panic!("expected title Label first, got {other:?}"),
        }
        assert_eq!(header.buttons[1], ToolbarButton::Spacer);

        let tb = panel_layout
            .title_bar_bounds
            .expect("title bar present when title is Some");
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

        let mut header_action_bounds: HashMap<WidgetId, Rect> = HashMap::new();
        for vis in &header_layout.visible_items {
            if let ToolbarButton::Action { id, .. } = &header.buttons[vis.item_idx] {
                header_action_bounds.insert(*id, vis.bounds);
            }
        }

        assert_eq!(
            header_action_bounds.len(),
            panel_layout.visible_actions.len(),
            "header paints exactly the actions Panel::layout kept, no more, no fewer"
        );
        for va in &panel_layout.visible_actions {
            let hb = header_action_bounds
                .get(&va.id)
                .unwrap_or_else(|| panic!("action {:?} missing from header layout", va.id));
            assert_eq!(*hb, va.bounds, "action {:?} bounds mismatch", va.id);
        }
    }

    #[test]
    fn header_toolbar_matches_layout_tui_measure() {
        // TUI-shaped measure: 1 cell title bar, 3-cell action buttons.
        let measure = PanelMeasure {
            title_bar_height: 1.0,
            action_button_width: 3.0,
            content_padding: 0.0,
        };
        let bounds = Rect::new(0.0, 0.0, 10.0, 5.0);
        // 0, 1, 2 actions all fit (<= 6 of the 10-wide title bar); 5
        // actions overflow (15 > 10) and only the first 3 fit.
        for action_count in [0, 1, 2, 5] {
            assert_header_matches_layout(bounds, measure, action_count);
        }
    }

    #[test]
    fn header_toolbar_matches_layout_pixel_measure() {
        // Pixel-shaped measure: 24px title bar, 24px action buttons.
        let measure = PanelMeasure {
            title_bar_height: 24.0,
            action_button_width: 24.0,
            content_padding: 0.0,
        };
        let bounds = Rect::new(0.0, 0.0, 80.0, 100.0);
        // 0, 1, 2 actions fit (<= 48 of the 80-wide title bar); 5
        // actions overflow (120 > 80) and only the first 3 fit.
        for action_count in [0, 1, 2, 5] {
            assert_header_matches_layout(bounds, measure, action_count);
        }
    }

    #[test]
    fn header_toolbar_none_without_title() {
        let p = Panel {
            id: WidgetId::new("p"),
            title: None,
            actions: vec![mk_panel_action("close", '×')],
            accent: None,
            collapsed: false,
        };
        let bounds = Rect::new(0.0, 0.0, 200.0, 100.0);
        assert!(p.header_toolbar(bounds, PanelMeasure::new(20.0)).is_none());
    }

    #[test]
    fn header_toolbar_present_when_collapsed() {
        // Mapping changes paint, not layout: a collapsed panel still
        // has a title bar, so still maps to a header toolbar.
        let p = Panel {
            id: WidgetId::new("p"),
            title: Some(StyledText::plain("Collapsed")),
            actions: vec![],
            accent: None,
            collapsed: true,
        };
        let bounds = Rect::new(0.0, 0.0, 200.0, 100.0);
        assert!(p.header_toolbar(bounds, PanelMeasure::new(20.0)).is_some());
    }
}
