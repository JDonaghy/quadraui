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
    /// When true, a vertical scrollbar is rendered at the trailing edge
    /// of the viewport. The body's visible width shrinks by the scrollbar
    /// gutter width (1 cell on TUI, ~12px on GTK). The scrollbar's
    /// thumb + track hit regions are included in the layout's
    /// `hit_regions` for drag interaction.
    #[serde(default)]
    pub show_scrollbar: bool,
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
    ScrollbarThumb,
    ScrollbarTrackBefore,
    ScrollbarTrackAfter,
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
    /// Scrollbar gutter bounds (when `show_scrollbar` is true).
    pub scrollbar_bounds: Option<Rect>,
    /// Scrollbar thumb bounds within the gutter.
    pub thumb_bounds: Option<Rect>,
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
        self.layout_inner(viewport_width, viewport_height, 0.0, 0.0, measure_line)
    }

    /// Layout with scrollbar gutter reserved on the right.
    /// `scrollbar_gutter` is the width in native units (1.0 for TUI,
    /// ~12.0 for GTK). `min_thumb` is the minimum thumb length.
    pub fn layout_with_scrollbar<F>(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        scrollbar_gutter: f32,
        min_thumb: f32,
        measure_line: F,
    ) -> TextDisplayLayout
    where
        F: Fn(usize) -> TextDisplayLineMeasure,
    {
        self.layout_inner(
            viewport_width,
            viewport_height,
            scrollbar_gutter,
            min_thumb,
            measure_line,
        )
    }

    fn layout_inner<F>(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        scrollbar_gutter: f32,
        min_thumb: f32,
        measure_line: F,
    ) -> TextDisplayLayout
    where
        F: Fn(usize) -> TextDisplayLineMeasure,
    {
        let mut visible_lines: Vec<VisibleTextDisplayLine> = Vec::new();
        let mut hit_regions: Vec<(Rect, TextDisplayHit)> = Vec::new();

        let body_width = if self.show_scrollbar && scrollbar_gutter > 0.0 {
            (viewport_width - scrollbar_gutter).max(0.0)
        } else {
            viewport_width
        };

        if self.lines.is_empty() || viewport_height <= 0.0 {
            return TextDisplayLayout {
                viewport_width,
                viewport_height,
                visible_lines,
                hit_regions,
                resolved_scroll_offset: 0,
                scrollbar_bounds: None,
                thumb_bounds: None,
            };
        }

        // Decide the starting offset.
        let resolved_scroll_offset = if self.auto_scroll {
            let mut used = 0.0_f32;
            let mut offset = self.lines.len();
            while offset > 0 {
                let cand = offset - 1;
                let h = measure_line(cand).height;
                if used + h > viewport_height + f32::EPSILON {
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
            let bounds = Rect::new(0.0, y, body_width, height);
            visible_lines.push(VisibleTextDisplayLine {
                line_idx: i,
                bounds,
            });
            hit_regions.push((bounds, TextDisplayHit::Line(i)));
            y += m.height;
        }

        // Scrollbar.
        let (scrollbar_bounds, thumb_bounds) =
            if self.show_scrollbar && scrollbar_gutter > 0.0 && !self.lines.is_empty() {
                let gutter = Rect::new(body_width, 0.0, scrollbar_gutter, viewport_height);
                let visible_count = visible_lines.len() as f32;
                let total = self.lines.len() as f32;
                let (thumb_start, thumb_len) = crate::primitives::scrollbar::fit_thumb(
                    resolved_scroll_offset as f32,
                    total,
                    visible_count,
                    viewport_height,
                    min_thumb,
                );

                if thumb_len > 0.0 {
                    let thumb = Rect::new(body_width, thumb_start, scrollbar_gutter, thumb_len);
                    let track_before = Rect::new(body_width, 0.0, scrollbar_gutter, thumb_start);
                    let track_after = Rect::new(
                        body_width,
                        thumb_start + thumb_len,
                        scrollbar_gutter,
                        (viewport_height - thumb_start - thumb_len).max(0.0),
                    );

                    hit_regions.insert(0, (thumb, TextDisplayHit::ScrollbarThumb));
                    if track_before.height > 0.0 {
                        hit_regions.insert(1, (track_before, TextDisplayHit::ScrollbarTrackBefore));
                    }
                    if track_after.height > 0.0 {
                        hit_regions.insert(
                            if track_before.height > 0.0 { 2 } else { 1 },
                            (track_after, TextDisplayHit::ScrollbarTrackAfter),
                        );
                    }
                    (Some(gutter), Some(thumb))
                } else {
                    (Some(gutter), None)
                }
            } else {
                (None, None)
            };

        TextDisplayLayout {
            viewport_width,
            viewport_height,
            visible_lines,
            hit_regions,
            resolved_scroll_offset,
            scrollbar_bounds,
            thumb_bounds,
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
            show_scrollbar: false,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StyledSpan;

    fn line(text: &str) -> TextDisplayLine {
        TextDisplayLine {
            spans: vec![StyledSpan::plain(text)],
            decoration: Decoration::Normal,
            timestamp: None,
        }
    }

    fn make_td(lines: usize, show_scrollbar: bool, scroll: usize, auto: bool) -> TextDisplay {
        TextDisplay {
            id: WidgetId::new("td"),
            lines: (0..lines).map(|i| line(&format!("line{i}"))).collect(),
            scroll_offset: scroll,
            auto_scroll: auto,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar,
        }
    }

    #[test]
    fn scrollbar_layout_reserves_gutter_width() {
        let td = make_td(20, true, 0, false);
        let layout =
            td.layout_with_scrollbar(40.0, 10.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        // Body width should be 39 (40 - 1 gutter).
        for vis in &layout.visible_lines {
            assert!(
                (vis.bounds.width - 39.0).abs() < 0.01,
                "line body width should be 39, got {}",
                vis.bounds.width
            );
        }
        assert!(layout.scrollbar_bounds.is_some());
        let gutter = layout.scrollbar_bounds.unwrap();
        assert!((gutter.x - 39.0).abs() < 0.01);
        assert!((gutter.width - 1.0).abs() < 0.01);
    }

    #[test]
    fn scrollbar_thumb_at_top_when_scroll_zero() {
        let td = make_td(20, true, 0, false);
        let layout =
            td.layout_with_scrollbar(40.0, 10.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        let thumb = layout.thumb_bounds.expect("thumb present");
        assert!(
            thumb.y.abs() < 0.01,
            "thumb should start at top, got y={}",
            thumb.y
        );
    }

    #[test]
    fn scrollbar_thumb_at_bottom_when_fully_scrolled() {
        let td = make_td(20, true, 10, false);
        let layout =
            td.layout_with_scrollbar(40.0, 10.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        let thumb = layout.thumb_bounds.expect("thumb present");
        assert!(
            (thumb.y + thumb.height - 10.0).abs() < 0.01,
            "thumb should touch bottom: y={}, h={}, viewport=10",
            thumb.y,
            thumb.height
        );
    }

    #[test]
    fn scrollbar_hit_test_regions_present() {
        let td = make_td(20, true, 5, false);
        let layout =
            td.layout_with_scrollbar(40.0, 10.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        // Scrollbar hit regions should be present (thumb + track before/after).
        let has_thumb = layout
            .hit_regions
            .iter()
            .any(|(_, h)| matches!(h, TextDisplayHit::ScrollbarThumb));
        let has_track_before = layout
            .hit_regions
            .iter()
            .any(|(_, h)| matches!(h, TextDisplayHit::ScrollbarTrackBefore));
        let has_track_after = layout
            .hit_regions
            .iter()
            .any(|(_, h)| matches!(h, TextDisplayHit::ScrollbarTrackAfter));
        assert!(has_thumb, "thumb hit region missing");
        assert!(has_track_before, "track-before hit region missing");
        assert!(has_track_after, "track-after hit region missing");
    }

    #[test]
    fn no_scrollbar_when_content_fits() {
        let td = make_td(5, true, 0, false);
        let layout =
            td.layout_with_scrollbar(40.0, 10.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        // Content fits in viewport; scrollbar should have no thumb.
        assert!(layout.thumb_bounds.is_none());
    }

    #[test]
    fn no_scrollbar_when_disabled() {
        let td = make_td(20, false, 0, false);
        let layout = td.layout(40.0, 10.0, |_| TextDisplayLineMeasure::new(1.0));
        assert!(layout.scrollbar_bounds.is_none());
        assert!(layout.thumb_bounds.is_none());
        // No scrollbar hit regions.
        let scrollbar_hits = layout.hit_regions.iter().any(|(_, h)| {
            matches!(
                h,
                TextDisplayHit::ScrollbarThumb
                    | TextDisplayHit::ScrollbarTrackBefore
                    | TextDisplayHit::ScrollbarTrackAfter
            )
        });
        assert!(!scrollbar_hits, "no scrollbar hits when disabled");
    }

    #[test]
    fn hit_test_line_in_body_area() {
        let td = make_td(20, true, 0, false);
        let layout =
            td.layout_with_scrollbar(40.0, 10.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        // Click in the body area (x=5, y=3.5) should hit line 3.
        match layout.hit_test(5.0, 3.5) {
            TextDisplayHit::Line(idx) => assert_eq!(idx, 3),
            other => panic!("expected Line(3), got {:?}", other),
        }
    }

    #[test]
    fn hit_test_scrollbar_thumb() {
        let td = make_td(20, true, 0, false);
        let layout =
            td.layout_with_scrollbar(40.0, 10.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        let thumb = layout.thumb_bounds.expect("thumb present");
        // Click inside the thumb.
        match layout.hit_test(thumb.x + 0.5, thumb.y + thumb.height / 2.0) {
            TextDisplayHit::ScrollbarThumb => {}
            other => panic!("expected ScrollbarThumb, got {:?}", other),
        }
    }

    #[test]
    fn auto_scroll_with_scrollbar() {
        let td = make_td(20, true, 0, true);
        let layout =
            td.layout_with_scrollbar(40.0, 10.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        // Auto-scroll should pin to bottom: resolved offset = 10.
        assert_eq!(layout.resolved_scroll_offset, 10);
        assert_eq!(layout.visible_lines.last().unwrap().line_idx, 19);
    }

    #[test]
    fn append_line_and_max_lines() {
        let mut td = TextDisplay::new(WidgetId::new("td"));
        td.set_max_lines(5);
        for i in 0..10 {
            td.append_line(line(&format!("l{i}")));
        }
        assert_eq!(td.lines.len(), 5);
        // Oldest lines should be trimmed; newest 5 remain.
        assert_eq!(td.lines[0].spans[0].text, "l5");
        assert_eq!(td.lines[4].spans[0].text, "l9");
    }
}
