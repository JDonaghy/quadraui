//! `TextDisplay` primitive: a scrollable, append-only viewer for streamed
//! text. Distinct from `Terminal` (which is VT100-aware with cursor and
//! attributes) and from `TextEditor` (deferred to A.9 — full editor with
//! cursor, selection, undo). `TextDisplay` is the right primitive for
//! log tails, command output, debug console, kubectl logs streams.
//!
//! The primitive itself is a `Vec<TextDisplayLine>` plus scroll + auto-
//! scroll state. Backends are expected to render efficiently — for
//! high-volume streams (≥10k lines/sec target per #144) backends may
//! diff only the appended slice rather than re-rasterising the whole
//! viewport. The primitive's append-only API (`append_line`, no
//! mid-buffer mutation) is what makes that diff cheap.
//!
//! **Status:** A.8 ships the primitive types only. Backend draw
//! functions and optimised partial-repaint paths land when the first
//! consumer (kubectl logs viewer #145, LSP trace viewer, etc.) needs
//! them.
//!
//! # Backend contract
//!
//! **Declarative + auto-scroll convention.** Render
//! `lines[scroll_offset..]` from top to bottom of the viewport. Each
//! `TextDisplayLine` carries pre-styled spans + an optional `decoration`
//! (info / warn / error tint) + an optional `timestamp`. Backends paint
//! the spans, optionally tint the row by decoration, optionally render
//! the timestamp prefix in a dim style.
//!
//! **Auto-scroll handling**: when `auto_scroll == true`, the backend
//! ignores `scroll_offset` and pins the view to the bottom (newest
//! lines). When the user scrolls up, the backend (or the app on its
//! behalf) sets `auto_scroll = false` and respects `scroll_offset`
//! until the user scrolls back to the bottom.
//!
//! **Performance**: for high-volume streams, backends may diff only the
//! newly-appended lines (the primitive is append-only — `append_line` /
//! `clear` / cap-eviction are the only mutations) and repaint just the
//! affected rows. Reference implementations land with the first
//! consumer.

use crate::event::Rect;
use crate::types::{Decoration, Modifiers, StyledSpan, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a `TextDisplay`.
///
/// Lines are rendered top-to-bottom in insertion order. `scroll_offset`
/// is the index of the first visible line (0 = top). When `auto_scroll`
/// is true the backend should clamp `scroll_offset` to keep the most
/// recent line visible after each `append_line` — paused only when the
/// user explicitly scrolls upward.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextDisplay {
    pub id: WidgetId,
    pub lines: Vec<TextDisplayLine>,
    /// Index of the first visible line. `0` = top.
    #[serde(default)]
    pub scroll_offset: usize,
    /// When true, backends auto-scroll to keep the latest line visible.
    /// Toggled off when the user scrolls upward, re-enabled when they
    /// scroll back to the bottom.
    #[serde(default = "default_auto_scroll")]
    pub auto_scroll: bool,
    /// Maximum lines to retain in the ring buffer. `0` = unbounded.
    /// Helpful for log tails where memory can grow without bound.
    #[serde(default)]
    pub max_lines: usize,
    #[serde(default)]
    pub has_focus: bool,
    /// Optional title row painted above the body. The body's visible
    /// height shrinks by one row when present; spans render as-is so
    /// callers control colour/bold/etc. Backends consume this directly
    /// — no bespoke title painter needed.
    #[serde(default)]
    pub title: Option<StyledText>,
}

fn default_auto_scroll() -> bool {
    true
}

/// One line in a `TextDisplay`. Carries styled spans plus an optional
/// decoration tag (Error/Warning/Muted/Header) for log-level styling and
/// an optional left-aligned timestamp string the backend renders in a
/// dim colour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextDisplayLine {
    pub spans: Vec<StyledSpan>,
    #[serde(default)]
    pub decoration: Decoration,
    /// Optional timestamp prefix (e.g. `"12:34:56"`) rendered before spans.
    #[serde(default)]
    pub timestamp: Option<String>,
}

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// Per Decision D6: primitives return fully-resolved `Layout` structs.
// Eighth primitive on the new shape. TextDisplay is a vertical stack
// of log lines, with auto-scroll support: when `auto_scroll` is true,
// the layout pins to the bottom (newest lines visible) regardless of
// the input `scroll_offset`. The backend doesn't need to compute this
// itself — `resolved_scroll_offset` is correct either way.

/// Per-line measurement supplied by the backend. Single-line displays
/// usually have a uniform `height`, but wrap-enabled backends can vary
/// it (e.g. a long line that wraps onto 3 visual rows returns `3.0`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextDisplayLineMeasure {
    pub height: f32,
}

impl TextDisplayLineMeasure {
    pub fn new(height: f32) -> Self {
        Self { height }
    }
}

/// Resolved position of one visible text-display line after layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleTextDisplayLine {
    /// Index into `TextDisplay.lines`.
    pub line_idx: usize,
    pub bounds: Rect,
}

/// Classification of a hit-test result. Clicks on log lines usually
/// start a text-selection or copy action; the primitive reports which
/// line was hit and the backend decides what to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextDisplayHit {
    Line(usize),
    Empty,
}

/// Fully-resolved text-display layout.
#[derive(Debug, Clone, PartialEq)]
pub struct TextDisplayLayout {
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub visible_lines: Vec<VisibleTextDisplayLine>,
    pub hit_regions: Vec<(Rect, TextDisplayHit)>,
    /// Scroll offset actually used. When `auto_scroll` is true, this is
    /// chosen so the last line is visible; otherwise it's the input
    /// `scroll_offset` clamped to `[0, lines.len())`. Backends should
    /// write this back to the app's stored value so auto-scroll state
    /// is coherent across frames.
    pub resolved_scroll_offset: usize,
}

impl TextDisplayLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> TextDisplayHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        TextDisplayHit::Empty
    }
}

impl TextDisplay {
    /// Compute the full rendering + hit-test layout for this display.
    ///
    /// # Auto-scroll
    ///
    /// When `self.auto_scroll == true`, the layout chooses the
    /// smallest `resolved_scroll_offset` such that the last line is
    /// still visible — overriding the stored `scroll_offset`. When
    /// `auto_scroll == false`, `scroll_offset` is used as-is (clamped).
    ///
    /// # Arguments
    ///
    /// - `viewport_width`, `viewport_height` — display area.
    /// - `measure_line(i)` — height for line `i`. Most backends use a
    ///   uniform height; wrap-enabled renderers return the wrapped-line
    ///   row count × base height.
    pub fn layout<F>(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        measure_line: F,
    ) -> TextDisplayLayout
    where
        F: Fn(usize) -> TextDisplayLineMeasure,
    {
        let mut visible_lines: Vec<VisibleTextDisplayLine> = Vec::new();
        let mut hit_regions: Vec<(Rect, TextDisplayHit)> = Vec::new();

        if self.lines.is_empty() || viewport_height <= 0.0 {
            return TextDisplayLayout {
                viewport_width,
                viewport_height,
                visible_lines,
                hit_regions,
                resolved_scroll_offset: 0,
            };
        }

        // Decide the starting offset.
        let resolved_scroll_offset = if self.auto_scroll {
            // Walk backwards from the last line accumulating heights
            // until we've filled (or would overflow) the viewport.
            let mut used = 0.0_f32;
            let mut offset = self.lines.len();
            while offset > 0 {
                let cand = offset - 1;
                let h = measure_line(cand).height;
                if used + h > viewport_height + f32::EPSILON {
                    // One more line past the top → walking back further
                    // overshoots. Stop here.
                    break;
                }
                used += h;
                offset = cand;
            }
            offset
        } else {
            self.scroll_offset.min(self.lines.len() - 1)
        };

        let mut y = 0.0_f32;
        for i in resolved_scroll_offset..self.lines.len() {
            if y >= viewport_height {
                break;
            }
            let m = measure_line(i);
            let remaining = viewport_height - y;
            let height = m.height.min(remaining).max(0.0);
            if height <= 0.0 {
                break;
            }
            let bounds = Rect::new(0.0, y, viewport_width, height);
            visible_lines.push(VisibleTextDisplayLine {
                line_idx: i,
                bounds,
            });
            hit_regions.push((bounds, TextDisplayHit::Line(i)));
            y += m.height;
        }

        TextDisplayLayout {
            viewport_width,
            viewport_height,
            visible_lines,
            hit_regions,
            resolved_scroll_offset,
        }
    }
}

/// Events a `TextDisplay` emits back to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextDisplayEvent {
    /// User scrolled the view (mouse wheel, PageUp/Down, etc.).
    /// `new_offset` is the post-scroll `scroll_offset`. Apps update
    /// `auto_scroll` based on whether the new offset reached the bottom.
    Scrolled { new_offset: usize },
    /// User toggled auto-scroll (typically via a keyboard shortcut or
    /// click on a "Follow" indicator).
    AutoScrollToggled { enabled: bool },
    /// User initiated a copy of selected lines.
    Copied { text: String },
    /// A key was pressed with the display focused and the primitive
    /// did not consume it.
    KeyPressed { key: String, modifiers: Modifiers },
}

impl TextDisplay {
    /// Construct an empty `TextDisplay` with the given id.
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            lines: Vec::new(),
            scroll_offset: 0,
            auto_scroll: true,
            max_lines: 0,
            has_focus: false,
            title: None,
        }
    }

    /// Append a line to the end of the buffer. Honours `max_lines` by
    /// dropping the oldest line(s) when the buffer would grow past the cap.
    pub fn append_line(&mut self, line: TextDisplayLine) {
        self.lines.push(line);
        if self.max_lines > 0 && self.lines.len() > self.max_lines {
            let drop = self.lines.len() - self.max_lines;
            self.lines.drain(..drop);
            // Adjust scroll offset so the visible region stays put when
            // we evict older lines.
            self.scroll_offset = self.scroll_offset.saturating_sub(drop);
        }
    }

    /// Drop all lines and reset scroll to top.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.scroll_offset = 0;
    }

    /// Set the max retention; when set lower than the current line count,
    /// trims oldest lines immediately.
    pub fn set_max_lines(&mut self, max: usize) {
        self.max_lines = max;
        if max > 0 && self.lines.len() > max {
            let drop = self.lines.len() - max;
            self.lines.drain(..drop);
            self.scroll_offset = self.scroll_offset.saturating_sub(drop);
        }
    }
}
