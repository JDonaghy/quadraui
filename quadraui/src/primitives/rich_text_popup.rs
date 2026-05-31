//! `RichTextPopup` primitive: an interactive bordered popup with
//! styled multi-line content, optional scroll, optional clickable
//! links, and optional text selection. Used for LSP hover with
//! markdown bodies, error popups with links to documentation,
//! and similar "open document inside the editor" surfaces.
//!
//! # Why not Tooltip?
//!
//! [`Tooltip`][crate::Tooltip] is for *static* hint text that isn't
//! meant to be interacted with — it has no scroll, no selection, no
//! focus state. The editor-hover use case needs all three (long doc
//! strings scroll; users copy text out; keyboard navigation Tabs
//! through links). Splitting them keeps Tooltip's API simple for the
//! many simple consumers and gives this richer surface its own type.
//!
//! # Backend contract
//!
//! **Modal-ish overlay.** Render as a bordered box at the resolved
//! position. The popup intercepts clicks landing inside it
//! (selection drag, link clicks, focus). Clicks outside follow app
//! policy — typical pattern is "mouse motion outside dismisses
//! after a short delay; click outside dismisses immediately."
//!
//! Per-line content is supplied as [`StyledText`] — backends already
//! know how to render those. The primitive's job is layout (where
//! does the box go? which lines are visible after scroll?
//! scrollbar bounds?) plus hit-test (which line/col does this
//! click land on? which link?).
//!
//! # Tree-sitter syntax highlighting in code blocks
//!
//! The primitive doesn't parse markdown or call into tree-sitter —
//! adapters pre-resolve those into `StyledText` spans (one span per
//! contiguous run sharing colour + bold/italic). Code-block tokens
//! become spans with `fg = Some(syntax_color)`. The primitive just
//! paints what it's given.

use crate::event::Rect;
use crate::types::{Color, Modifiers, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a rich-text popup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RichTextPopup {
    pub id: WidgetId,
    /// One styled row per line. The styling carries colour + bold +
    /// italic + underline; backends should respect all four.
    pub lines: Vec<StyledText>,
    /// Raw text per line (parallel to `lines`). Used by [`Self::char_at`]
    /// to map a click position to a `(line, col)` for selection
    /// extraction. Backends don't render this directly.
    pub line_text: Vec<String>,
    /// Optional per-line font-size scale (parallel to `lines` when
    /// non-empty; missing entries default to `1.0`). Adapters set
    /// `> 1.0` for markdown heading rows so they render larger.
    /// Backends apply via Pango font scale attr (GTK) or skip (TUI
    /// can't change cell size mid-render).
    #[serde(default)]
    pub line_scales: Vec<f32>,
    /// Index of the topmost visible line (0 = no scroll).
    #[serde(default)]
    pub scroll_top: usize,
    /// Maximum number of lines visible at once. Determines scrollbar
    /// presence + thumb sizing. Apps choose a value; typical
    /// vimcode hover popup uses 20.
    pub max_visible_rows: usize,
    /// True when the popup has keyboard focus — backends should
    /// render a focused border colour (typically `theme.md_link`).
    #[serde(default)]
    pub has_focus: bool,
    /// Active selection, normalised so `(start_line, start_col) <=
    /// (end_line, end_col)`. Backends invert fg/bg for characters
    /// inside the range when painting.
    #[serde(default)]
    pub selection: Option<TextSelection>,
    /// Clickable link spans. Used by backends to underline focused
    /// link characters and to translate clicks to "open URL" intents.
    #[serde(default)]
    pub links: Vec<RichTextLink>,
    /// Index into `links` of the keyboard-focused link. Backends
    /// underline only this link's characters.
    #[serde(default)]
    pub focused_link: Option<usize>,
    /// Preferred placement relative to the anchor (above by default;
    /// flips to below when there's no room).
    #[serde(default)]
    pub placement: PopupPlacement,
    /// Border + content padding in cell/pixel units.
    #[serde(default)]
    pub padding: f32,
    /// Override foreground colour for default-styled text. `None` =
    /// theme `hover_fg`.
    #[serde(default)]
    pub fg: Option<Color>,
    /// Override background colour. `None` = theme `hover_bg`.
    #[serde(default)]
    pub bg: Option<Color>,
}

/// Preferred placement of the popup relative to its anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PopupPlacement {
    /// Above the anchor cell (default — matches vimcode editor hover).
    #[default]
    Above,
    /// Below the anchor cell.
    Below,
}

/// A normalised text selection inside a `RichTextPopup`.
///
/// Backends should ensure `start_line < end_line` or
/// `start_line == end_line && start_col <= end_col` before storing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSelection {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

impl TextSelection {
    /// True iff `(line, col)` is inside the (normalised) selection.
    /// `col` is the character column on `line`.
    pub fn contains(&self, line: usize, col: usize) -> bool {
        if self.start_line == self.end_line {
            line == self.start_line && col >= self.start_col && col < self.end_col
        } else if line == self.start_line {
            col >= self.start_col
        } else if line == self.end_line {
            col < self.end_col
        } else {
            line > self.start_line && line < self.end_line
        }
    }
}

/// A clickable link within the popup content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RichTextLink {
    /// Line index in `RichTextPopup.lines`.
    pub line: usize,
    /// Inclusive byte offset within `line_text[line]`.
    pub start_byte: usize,
    /// Exclusive byte offset within `line_text[line]`.
    pub end_byte: usize,
    /// URL or other target the app opens when the link is clicked.
    pub url: String,
}

/// Events a `RichTextPopup` emits back to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RichTextPopupEvent {
    /// User clicked a link; app should open `url` (browser, file, etc.).
    LinkActivated { idx: usize, url: String },
    /// Selection changed via drag — app stores the new value on
    /// `RichTextPopup.selection` for next render.
    SelectionChanged { value: Option<TextSelection> },
    /// Scroll offset changed (mouse wheel, scrollbar drag, keyboard).
    ScrollOffsetChanged { new_offset: usize },
    /// User dismissed the popup (Escape, click outside, blur).
    Closed,
    /// Key pressed while popup had focus and the primitive didn't
    /// consume it. Apps may handle e.g. PageUp/PageDown.
    KeyPressed { key: String, modifiers: Modifiers },
}

// ── D6 Layout API ───────────────────────────────────────────────────────────

/// Per-line measurement supplied by the backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RichTextPopupMeasure {
    /// Width of the popup CONTENT (without borders).
    pub content_width: f32,
    /// Height of one *unscaled* row in the backend's unit (cells / pixels).
    pub row_height: f32,
    /// Whether the backend renders per-line font scale (see
    /// [`RichTextPopup::line_scales`] and
    /// [`Backend::scales_text_rows`][crate::Backend::scales_text_rows]).
    /// When `true`, [`RichTextPopup::layout`] reserves
    /// `row_height * line_scales[i]` for each row so scaled headings
    /// don't overlap. When `false` (fixed-cell backends like TUI), every
    /// row is exactly `row_height` regardless of scale. Set it from
    /// `backend.scales_text_rows()` so consumer code stays
    /// backend-neutral.
    pub scale_rows: bool,
}

impl RichTextPopupMeasure {
    /// Construct a measure with `scale_rows = false` (fixed-height rows).
    /// Use [`Self::with_scale_rows`] to opt a scaling backend in.
    pub fn new(content_width: f32, row_height: f32) -> Self {
        Self {
            content_width,
            row_height,
            scale_rows: false,
        }
    }

    /// Set whether scaled rows reserve proportionally more height.
    /// Pass `backend.scales_text_rows()`.
    pub fn with_scale_rows(mut self, scale_rows: bool) -> Self {
        self.scale_rows = scale_rows;
        self
    }
}

/// Resolved position of one visible row inside the popup.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleRichTextLine {
    /// Index into `RichTextPopup.lines`.
    pub line_idx: usize,
    /// Bounds of the row in viewport coordinates.
    pub bounds: Rect,
}

/// Bounds of the scrollbar's track and thumb (when scrolling is needed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PopupScrollbar {
    pub track: Rect,
    pub thumb: Rect,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RichTextPopupHit {
    /// Click landed on a link — carries the link index.
    Link(usize),
    /// Click landed on a regular character — carries `(line, col)`.
    Char(usize, usize),
    /// Click landed on the scrollbar track outside the thumb (jump-scroll).
    ScrollbarTrack,
    /// Click landed on the scrollbar thumb (start drag).
    ScrollbarThumb,
    /// Click landed on the popup body but not a specific feature.
    Body,
    /// Click landed outside the popup.
    Outside,
}

/// Fully-resolved popup layout.
#[derive(Debug, Clone, PartialEq)]
pub struct RichTextPopupLayout {
    /// Full bounds of the popup box (incl border).
    pub bounds: Rect,
    /// Content area inside the borders (where lines render).
    pub content_bounds: Rect,
    /// Visible lines after applying `scroll_top` + `max_visible_rows`.
    pub visible_lines: Vec<VisibleRichTextLine>,
    /// Resolved scroll offset (clamped to valid range).
    pub resolved_scroll_offset: usize,
    /// Scrollbar bounds when content overflows; `None` otherwise.
    pub scrollbar: Option<PopupScrollbar>,
    /// Per-link character hit zones (computed from `links` + visible
    /// rows + measured char widths). Each entry is `(rect, link_idx)`.
    pub link_hit_regions: Vec<(Rect, usize)>,
}

impl RichTextPopupLayout {
    /// Hit-test a viewport position. Returns the most specific hit:
    /// link > scrollbar > char > body > outside.
    pub fn hit_test(&self, x: f32, y: f32) -> RichTextPopupHit {
        // Outside the box entirely.
        if x < self.bounds.x
            || x >= self.bounds.x + self.bounds.width
            || y < self.bounds.y
            || y >= self.bounds.y + self.bounds.height
        {
            return RichTextPopupHit::Outside;
        }
        // Link?
        for (rect, idx) in &self.link_hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return RichTextPopupHit::Link(*idx);
            }
        }
        // Scrollbar?
        if let Some(sb) = self.scrollbar {
            if x >= sb.thumb.x
                && x < sb.thumb.x + sb.thumb.width
                && y >= sb.thumb.y
                && y < sb.thumb.y + sb.thumb.height
            {
                return RichTextPopupHit::ScrollbarThumb;
            }
            if x >= sb.track.x
                && x < sb.track.x + sb.track.width
                && y >= sb.track.y
                && y < sb.track.y + sb.track.height
            {
                return RichTextPopupHit::ScrollbarTrack;
            }
        }
        // Body — caller can refine via `char_at` if it cares about (line, col).
        RichTextPopupHit::Body
    }

    /// Map a viewport position to the `(line, col)` of the character
    /// underneath. `col_width` is the backend's per-character advance
    /// (cell = 1, pixel font = char width). Returns `None` if outside
    /// any visible row.
    ///
    /// Used during selection drag — the app records selection start
    /// on mouse-down and updates the end on each mouse-move.
    pub fn char_at(&self, x: f32, y: f32, col_width: f32) -> Option<(usize, usize)> {
        for vis in &self.visible_lines {
            if y >= vis.bounds.y && y < vis.bounds.y + vis.bounds.height {
                let rel_x = (x - vis.bounds.x).max(0.0);
                let col = if col_width > 0.0 {
                    (rel_x / col_width) as usize
                } else {
                    0
                };
                return Some((vis.line_idx, col));
            }
        }
        None
    }
}

impl RichTextPopup {
    /// Compute the full popup layout at `(anchor_x, anchor_y)`.
    ///
    /// The anchor is typically the top-left of the editor cell the
    /// popup describes. Placement choice puts the popup above (with
    /// fallback to below) or below (with fallback to above) per
    /// `placement`.
    ///
    /// `viewport` clamps the popup; if both placements overflow,
    /// the popup is pinned to the viewport edge.
    ///
    /// `measure` supplies content-area width and per-row height.
    /// `link_widths(line, byte_range) -> width` returns the rendered
    /// width of an arbitrary substring on a line — used to compute
    /// link hit regions in pixel-unit backends. TUI passes
    /// `|_, range| (range.end - range.start) as f32`.
    pub fn layout<W>(
        &self,
        anchor_x: f32,
        anchor_y: f32,
        viewport: Rect,
        measure: RichTextPopupMeasure,
        link_widths: W,
    ) -> RichTextPopupLayout
    where
        W: Fn(usize, usize, usize) -> f32,
    {
        let total_lines = self.lines.len();
        let max_rows = self.max_visible_rows.max(1);
        // Clamp scroll FIRST so a stale `scroll_top` past `max_scroll`
        // still produces a valid visible window (last full screen).
        let max_scroll = total_lines.saturating_sub(max_rows);
        let resolved_scroll_offset = self.scroll_top.min(max_scroll);
        let visible_count = total_lines
            .saturating_sub(resolved_scroll_offset)
            .min(max_rows);

        // Per-row height. Fixed-cell backends (`scale_rows == false`,
        // e.g. TUI) keep every row at `row_height`; scaling backends
        // (GTK) reserve `row_height * line_scales[i]` so larger heading
        // glyphs don't overlap the rows below. Scales below 1.0 are
        // clamped so a stray small scale can't shrink a row.
        let row_height_at = |line_idx: usize| -> f32 {
            let scale = if measure.scale_rows {
                self.line_scales
                    .get(line_idx)
                    .copied()
                    .unwrap_or(1.0)
                    .max(1.0)
            } else {
                1.0
            };
            measure.row_height * scale
        };

        // Total content height = sum of the visible window's row heights
        // (varies with which rows are scrolled into view when scales differ).
        let content_h: f32 = (0..visible_count)
            .map(|i| row_height_at(resolved_scroll_offset + i))
            .sum();

        let pad = self.padding.max(0.0);
        let border = 1.0; // 1 cell / 1 pixel each side
        let outer_w = measure.content_width + pad * 2.0 + border * 2.0;
        let outer_h = content_h + pad * 2.0 + border * 2.0;

        // Placement: above the anchor when there's room, otherwise below.
        let prefer_above = self.placement == PopupPlacement::Above;
        let above_y = anchor_y - outer_h;
        let below_y = anchor_y + measure.row_height;
        let y = match (prefer_above, above_y >= viewport.y) {
            (true, true) => above_y,
            (true, false) => below_y,
            (false, _) if below_y + outer_h <= viewport.y + viewport.height => below_y,
            (false, _) => above_y.max(viewport.y),
        };
        // Clamp x so the popup stays inside viewport horizontally.
        let max_x = (viewport.x + viewport.width - outer_w).max(viewport.x);
        let x = anchor_x.clamp(viewport.x, max_x);
        // Clamp y similarly.
        let max_y = (viewport.y + viewport.height - outer_h).max(viewport.y);
        let y = y.clamp(viewport.y, max_y);

        let bounds = Rect::new(x, y, outer_w, outer_h);
        let content_bounds = Rect::new(
            x + border + pad,
            y + border + pad,
            measure.content_width,
            content_h,
        );

        // Visible lines — accumulate `y` by each row's (possibly scaled)
        // height so rows never overlap on scaling backends.
        let mut visible_lines: Vec<VisibleRichTextLine> = Vec::with_capacity(visible_count);
        let mut row_y = content_bounds.y;
        for i in 0..visible_count {
            let line_idx = resolved_scroll_offset + i;
            let row_h = row_height_at(line_idx);
            visible_lines.push(VisibleRichTextLine {
                line_idx,
                bounds: Rect::new(content_bounds.x, row_y, content_bounds.width, row_h),
            });
            row_y += row_h;
        }

        // Scrollbar (1 cell / pixel wide at the right border).
        let scrollbar = if total_lines > max_rows {
            let track = Rect::new(
                bounds.x + bounds.width - border,
                content_bounds.y,
                border,
                content_bounds.height,
            );
            let thumb_h = (content_bounds.height * (max_rows as f32 / total_lines as f32))
                .max(measure.row_height);
            let max_thumb_top = (content_bounds.height - thumb_h).max(0.0);
            let thumb_top_offset = if max_scroll == 0 {
                0.0
            } else {
                (resolved_scroll_offset as f32 / max_scroll as f32) * max_thumb_top
            };
            let thumb = Rect::new(track.x, track.y + thumb_top_offset, border, thumb_h);
            Some(PopupScrollbar { track, thumb })
        } else {
            None
        };

        // Per-link hit regions for clickable spans on visible rows.
        let mut link_hit_regions: Vec<(Rect, usize)> = Vec::new();
        for vis in &visible_lines {
            for (idx, link) in self.links.iter().enumerate() {
                if link.line != vis.line_idx {
                    continue;
                }
                let pre_w = link_widths(link.line, 0, link.start_byte);
                let span_w = link_widths(link.line, link.start_byte, link.end_byte);
                let rect = Rect::new(
                    vis.bounds.x + pre_w,
                    vis.bounds.y,
                    span_w,
                    vis.bounds.height,
                );
                link_hit_regions.push((rect, idx));
            }
        }

        RichTextPopupLayout {
            bounds,
            content_bounds,
            visible_lines,
            resolved_scroll_offset,
            scrollbar,
            link_hit_regions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StyledText;

    /// Build a popup with `n` body lines and the given per-line scales.
    fn popup_with_scales(scales: &[f32]) -> RichTextPopup {
        let n = scales.len();
        RichTextPopup {
            id: WidgetId::new("rtp:test"),
            lines: (0..n)
                .map(|i| StyledText::plain(format!("line{i}")))
                .collect(),
            line_text: (0..n).map(|i| format!("line{i}")).collect(),
            line_scales: scales.to_vec(),
            scroll_top: 0,
            max_visible_rows: 20,
            has_focus: false,
            selection: None,
            links: Vec::new(),
            focused_link: None,
            placement: PopupPlacement::Below,
            padding: 0.0,
            fg: None,
            bg: None,
        }
    }

    fn vp() -> Rect {
        Rect::new(0.0, 0.0, 1000.0, 1000.0)
    }

    #[test]
    fn scale_rows_false_keeps_flat_geometry() {
        // Even with heading scales present, a fixed-cell backend lays
        // every row out at exactly `row_height` (pre-fix behaviour).
        let popup = popup_with_scales(&[2.0, 1.5, 1.0]);
        let measure = RichTextPopupMeasure::new(50.0, 10.0); // scale_rows = false
        let layout = popup.layout(0.0, 0.0, vp(), measure, |_, s, e| (e - s) as f32);
        for (i, vis) in layout.visible_lines.iter().enumerate() {
            assert!(
                (vis.bounds.height - 10.0).abs() < f32::EPSILON,
                "row {i} height should be flat 10.0, got {}",
                vis.bounds.height
            );
            let expected_y = layout.content_bounds.y + i as f32 * 10.0;
            assert!(
                (vis.bounds.y - expected_y).abs() < f32::EPSILON,
                "row {i} y should be {expected_y}, got {}",
                vis.bounds.y
            );
        }
        // content height = 3 flat rows.
        assert!((layout.content_bounds.height - 30.0).abs() < f32::EPSILON);
    }

    #[test]
    fn scale_rows_true_reserves_proportional_height() {
        // H1 (2.0x) then body (1.0x): the first row is twice as tall and
        // the second row starts below it (no overlap).
        let popup = popup_with_scales(&[2.0, 1.0]);
        let measure = RichTextPopupMeasure::new(50.0, 10.0).with_scale_rows(true);
        let layout = popup.layout(0.0, 0.0, vp(), measure, |_, s, e| (e - s) as f32);

        let r0 = layout.visible_lines[0].bounds;
        let r1 = layout.visible_lines[1].bounds;
        assert!(
            (r0.height - 20.0).abs() < f32::EPSILON,
            "H1 row should be 2 * 10 = 20 tall, got {}",
            r0.height
        );
        assert!(
            (r1.y - (r0.y + r0.height)).abs() < f32::EPSILON,
            "body row must start exactly below the H1 row (y={}, expected {})",
            r1.y,
            r0.y + r0.height
        );
        assert!(
            (r1.height - 10.0).abs() < f32::EPSILON,
            "body row should be 1 * 10 = 10 tall, got {}",
            r1.height
        );
        // content height = 20 (H1) + 10 (body) = 30.
        assert!(
            (layout.content_bounds.height - 30.0).abs() < f32::EPSILON,
            "content height should sum scaled rows (30), got {}",
            layout.content_bounds.height
        );
    }

    #[test]
    fn scale_rows_true_sums_all_three_heading_levels() {
        // H1/H2/H3/body = 2.0/1.5/1.2/1.0 over a 10px base.
        let popup = popup_with_scales(&[2.0, 1.5, 1.2, 1.0]);
        let measure = RichTextPopupMeasure::new(50.0, 10.0).with_scale_rows(true);
        let layout = popup.layout(0.0, 0.0, vp(), measure, |_, s, e| (e - s) as f32);
        let heights: Vec<f32> = layout
            .visible_lines
            .iter()
            .map(|v| v.bounds.height)
            .collect();
        assert_eq!(heights.len(), 4);
        let expected = [20.0_f32, 15.0, 12.0, 10.0];
        for (got, want) in heights.iter().zip(expected) {
            assert!((got - want).abs() < 1e-4, "row height {got} != {want}");
        }
        // Cumulative y positions and total content height.
        let mut acc = layout.content_bounds.y;
        for (i, vis) in layout.visible_lines.iter().enumerate() {
            assert!((vis.bounds.y - acc).abs() < 1e-4, "row {i} y mismatch");
            acc += vis.bounds.height;
        }
        assert!((layout.content_bounds.height - 57.0).abs() < 1e-4); // 20+15+12+10
    }

    #[test]
    fn scale_below_one_is_clamped() {
        // A stray sub-1.0 scale must not shrink a row below row_height.
        let popup = popup_with_scales(&[0.5]);
        let measure = RichTextPopupMeasure::new(50.0, 10.0).with_scale_rows(true);
        let layout = popup.layout(0.0, 0.0, vp(), measure, |_, s, e| (e - s) as f32);
        assert!((layout.visible_lines[0].bounds.height - 10.0).abs() < f32::EPSILON);
    }

    #[test]
    fn missing_scale_entry_defaults_to_one() {
        // Fewer line_scales than lines: missing entries are unscaled.
        let mut popup = popup_with_scales(&[2.0]);
        popup.lines.push(StyledText::plain("line1"));
        popup.line_text.push("line1".to_string());
        // line_scales has only one entry; line 1 has no scale.
        let measure = RichTextPopupMeasure::new(50.0, 10.0).with_scale_rows(true);
        let layout = popup.layout(0.0, 0.0, vp(), measure, |_, s, e| (e - s) as f32);
        assert!((layout.visible_lines[0].bounds.height - 20.0).abs() < f32::EPSILON);
        assert!((layout.visible_lines[1].bounds.height - 10.0).abs() < f32::EPSILON);
    }
}
