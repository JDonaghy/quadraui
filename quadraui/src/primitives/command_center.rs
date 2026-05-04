//! `CommandCenter` primitive: a horizontal strip with back/forward nav
//! arrows and a clickable search box. Lives in the menu bar row,
//! centered between the menu labels and any trailing chrome (window
//! controls, etc.).

use crate::event::Rect;
use crate::types::WidgetId;
use serde::{Deserialize, Serialize};

/// Declarative description of a command center strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandCenter {
    pub id: WidgetId,
    pub back_enabled: bool,
    pub forward_enabled: bool,
    /// Text shown inside the search box (e.g. "🔍 project-name").
    /// Empty string hides the search box entirely.
    #[serde(default)]
    pub search_label: String,
}

/// Measurement for a `CommandCenter`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommandCenterMeasure {
    /// Width of each nav arrow slot.
    pub arrow_width: f32,
    /// Gap between arrows and between arrow group and search box.
    pub gap: f32,
    /// Width of the search box. `0.0` when `search_label` is empty.
    pub search_box_width: f32,
    /// Height of the command center (matches the row).
    pub height: f32,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandCenterHit {
    Back,
    Forward,
    SearchBox,
    Bar,
    Outside,
}

/// Fully-resolved command-center layout.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandCenterLayout {
    pub bounds: Rect,
    pub back_bounds: Option<Rect>,
    pub forward_bounds: Option<Rect>,
    pub search_bounds: Option<Rect>,
    pub hit_regions: Vec<(Rect, CommandCenterHit)>,
}

impl CommandCenterLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> CommandCenterHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        CommandCenterHit::Outside
    }
}

impl CommandCenter {
    /// Compute layout. The entire command center is centered within `bounds`.
    pub fn layout(&self, bounds: Rect, measure: CommandCenterMeasure) -> CommandCenterLayout {
        let content_width = measure.arrow_width * 2.0
            + measure.gap
            + if measure.search_box_width > 0.0 {
                measure.gap + measure.search_box_width
            } else {
                0.0
            };

        let center_x = bounds.x + (bounds.width - content_width).max(0.0) / 2.0;
        let y = bounds.y;
        let h = measure.height;

        let back_rect = Rect::new(center_x, y, measure.arrow_width, h);
        let fwd_rect = Rect::new(
            center_x + measure.arrow_width + measure.gap,
            y,
            measure.arrow_width,
            h,
        );

        let search_rect = if measure.search_box_width > 0.0 {
            Some(Rect::new(
                fwd_rect.x + fwd_rect.width + measure.gap,
                y,
                measure.search_box_width,
                h,
            ))
        } else {
            None
        };

        let mut hit_regions = Vec::new();
        hit_regions.push((back_rect, CommandCenterHit::Back));
        hit_regions.push((fwd_rect, CommandCenterHit::Forward));
        if let Some(sb) = search_rect {
            hit_regions.push((sb, CommandCenterHit::SearchBox));
        }
        hit_regions.push((bounds, CommandCenterHit::Bar));

        CommandCenterLayout {
            bounds,
            back_bounds: Some(back_rect),
            forward_bounds: Some(fwd_rect),
            search_bounds: search_rect,
            hit_regions,
        }
    }
}
