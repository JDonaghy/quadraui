//! `Spinner` primitive: an indeterminate activity indicator. Used for
//! ongoing operations whose duration isn't known ahead of time —
//! LSP boot, git clone, Mason installs, project search.
//!
//! The spinner carries a `frame_idx` (app-incremented per tick) and a
//! label. Backends pick the glyph from their own frame table using
//! `frame_idx % frame_count`. TUI commonly uses braille frames
//! (`⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`), GUI backends may use a rotating arc or native
//! equivalent.
//!
//! # Backend contract
//!
//! **Declarative snapshot.** The app advances `frame_idx` on its own
//! animation ticker (~100 ms for braille, faster for arcs). The
//! primitive has no built-in timer — it's a paint description only.
//! Backends rasterise the current glyph + label.

use crate::event::Rect;
use crate::types::{Color, Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of an indeterminate spinner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spinner {
    pub id: WidgetId,
    /// Label shown next to the spinner, e.g. "Indexing…". Empty =
    /// glyph only.
    #[serde(default)]
    pub label: String,
    /// Animation frame index. Apps increment this on their ticker;
    /// backends render `frame_idx % frame_count` of their own frame
    /// table.
    #[serde(default)]
    pub frame_idx: usize,
    /// Optional accent colour for the glyph. `None` = backend decides.
    #[serde(default)]
    pub accent: Option<Color>,
}

/// Events a `Spinner` emits. Spinners are read-only from the user's
/// perspective; the only event is `KeyPressed` (which rarely fires —
/// spinners don't take focus). `Cancelled` is emitted by
/// [`ProgressBar`](super::progress::ProgressBar), not here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpinnerEvent {
    KeyPressed { key: String, modifiers: Modifiers },
}

// ── D6 Layout API ───────────────────────────────────────────────────────────

/// Measurement for a `Spinner`. Backends report the inline width of
/// the glyph + label at current font metrics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpinnerMeasure {
    pub width: f32,
    pub height: f32,
}

impl SpinnerMeasure {
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// Classification of a hit-test result. Spinners are not interactive —
/// only `Body` / `Empty`. `Body` is returned so apps can still use a
/// spinner click as "focus the originating operation" if they want.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpinnerHit {
    Body(WidgetId),
    Empty,
}

/// Fully-resolved spinner layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpinnerLayout {
    pub bounds: Rect,
}

impl SpinnerLayout {
    pub fn hit_test(&self, x: f32, y: f32, id: &WidgetId) -> SpinnerHit {
        if x >= self.bounds.x
            && x < self.bounds.x + self.bounds.width
            && y >= self.bounds.y
            && y < self.bounds.y + self.bounds.height
        {
            SpinnerHit::Body(id.clone())
        } else {
            SpinnerHit::Empty
        }
    }
}

impl Spinner {
    /// Compute the spinner's bounds given backend measurement.
    ///
    /// # Arguments
    ///
    /// - `origin_x`, `origin_y` — top-left position in the parent
    ///   surface's coordinate space.
    /// - `measure` — width/height for the current `label` at font
    ///   metrics (TUI: glyph + space + label chars; GTK: Pango).
    pub fn layout(&self, origin_x: f32, origin_y: f32, measure: SpinnerMeasure) -> SpinnerLayout {
        SpinnerLayout {
            bounds: Rect::new(origin_x, origin_y, measure.width, measure.height),
        }
    }
}
