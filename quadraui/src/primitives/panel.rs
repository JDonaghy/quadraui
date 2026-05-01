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
//! buttons + border; leave `content_bounds` to the app. Clicks on
//! action buttons emit `PanelEvent::ActionClicked { id }`; clicks on
//! the title bar emit `TitleBarClicked` (apps may use this for
//! drag-to-move or focus); clicks on content bounds emit
//! `ContentClicked` so the app can route further.

use crate::event::Rect;
use crate::types::{Color, Modifiers, StyledText, WidgetId};
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

/// Events a `Panel` emits back to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PanelEvent {
    /// User clicked an action button in the title bar.
    ActionClicked { id: WidgetId },
    /// User clicked the title bar body (not on an action).
    TitleBarClicked { id: WidgetId },
    /// User clicked anywhere in the content area. Apps handle further
    /// routing.
    ContentClicked { id: WidgetId },
    /// Key pressed while the panel had focus and the primitive didn't
    /// consume it.
    KeyPressed { key: String, modifiers: Modifiers },
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
