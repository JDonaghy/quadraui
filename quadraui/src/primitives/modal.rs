//! `Modal` primitive: a centered, backdrop-covered overlay region for
//! custom modal content. Distinct from `Dialog` (title + body +
//! buttons — fixed layout) and `Palette` (query + filterable list).
//! Use `Modal` for free-form modal surfaces: "Create new file" (Form
//! inside), keyboard-shortcut cheatsheet (TextDisplay inside), etc.
//!
//! The modal primitive only describes the frame: where the centered
//! content sits and where the backdrop covers. Apps draw their
//! content into `content_bounds` and use the `backdrop_bounds` to
//! intercept clicks that should dismiss the modal.
//!
//! # Backend contract
//!
//! **Highest z-order overlay.** Render the backdrop (typically a
//! translucent dark fill) then the centered content box. Clicks
//! anywhere on the backdrop emit `BackdropClicked` — apps may
//! dismiss or ignore. Clicks inside the content region emit
//! `ContentClicked`; apps route further.

use crate::event::Rect;
use crate::types::{Color, Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a modal overlay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modal {
    pub id: WidgetId,
    /// Requested content width in the backend's native unit.
    pub content_width: u32,
    /// Requested content height.
    pub content_height: u32,
    /// Optional backdrop override (typically a translucent fill).
    /// `None` = theme default.
    #[serde(default)]
    pub backdrop_color: Option<Color>,
    /// When true, clicking the backdrop should dismiss (primitive
    /// itself doesn't enforce this — apps listen to `BackdropClicked`
    /// and decide). Defaults to `true`.
    #[serde(default = "default_dismiss_on_backdrop")]
    pub dismiss_on_backdrop: bool,
}

fn default_dismiss_on_backdrop() -> bool {
    true
}

/// Events a `Modal` emits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModalEvent {
    /// Click landed on the backdrop. Apps typically dismiss the modal
    /// iff `dismiss_on_backdrop` is true.
    BackdropClicked { id: WidgetId },
    /// Click landed on the content region (but not on a nested widget
    /// that consumed it).
    ContentClicked { id: WidgetId },
    /// Key pressed while the modal had focus and the primitive didn't
    /// consume it.
    KeyPressed { key: String, modifiers: Modifiers },
}

// ── D6 Layout API ───────────────────────────────────────────────────────────

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModalHit {
    /// Click landed inside the content box.
    Content(WidgetId),
    /// Click landed on the backdrop (between content box and viewport
    /// edge). Apps typically dismiss.
    Backdrop(WidgetId),
}

/// Fully-resolved modal layout.
#[derive(Debug, Clone, PartialEq)]
pub struct ModalLayout {
    /// Full viewport covered by the backdrop.
    pub backdrop_bounds: Rect,
    /// Centered content region.
    pub content_bounds: Rect,
    pub hit_regions: Vec<(Rect, ModalHit)>,
}

impl ModalLayout {
    /// Hit-test — always returns either Content or Backdrop; Modal
    /// covers the full viewport so Empty isn't possible.
    pub fn hit_test(&self, x: f32, y: f32) -> ModalHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        // Modal covers viewport; outside backdrop is outside the
        // viewport (unusual), treat as backdrop for safety.
        ModalHit::Backdrop(WidgetId::new(""))
    }
}

impl Modal {
    /// Compute modal positioning.
    ///
    /// # Arguments
    ///
    /// - `viewport` — the full backdrop area. The content is centered
    ///   inside this.
    ///
    /// The content box is clamped to fit inside the viewport — if the
    /// requested width/height exceeds the viewport, the content shrinks
    /// to `viewport.width` / `viewport.height`.
    pub fn layout(&self, viewport: Rect) -> ModalLayout {
        let cw = (self.content_width as f32).min(viewport.width);
        let ch = (self.content_height as f32).min(viewport.height);
        let cx = viewport.x + (viewport.width - cw) * 0.5;
        let cy = viewport.y + (viewport.height - ch) * 0.5;
        let content_bounds = Rect::new(cx, cy, cw, ch);

        // Hit regions: content first (content_bounds is inside backdrop),
        // then backdrop covering the rest.
        let hit_regions: Vec<(Rect, ModalHit)> = vec![
            (content_bounds, ModalHit::Content(self.id.clone())),
            (viewport, ModalHit::Backdrop(self.id.clone())),
        ];

        ModalLayout {
            backdrop_bounds: viewport,
            content_bounds,
            hit_regions,
        }
    }
}
