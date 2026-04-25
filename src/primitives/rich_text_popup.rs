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
    /// Height of one rendered row in the backend's unit (cells / pixels).
    pub row_height: f32,
}

impl RichTextPopupMeasure {
    pub fn new(content_width: f32, row_height: f32) -> Self {
        Self {
            content_width,
            row_height,
        }
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
            if x >= rect.x
                && x < rect.x + rect.width
                && y >= rect.y
                && y < rect.y + rect.height
            {
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
        let display_rows = total_lines.min(max_rows);

        let pad = self.padding.max(0.0);
        let border = 1.0; // 1 cell / 1 pixel each side
        let outer_w = measure.content_width + pad * 2.0 + border * 2.0;
        let outer_h = display_rows as f32 * measure.row_height + pad * 2.0 + border * 2.0;

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
            display_rows as f32 * measure.row_height,
        );

        // Visible lines.
        let mut visible_lines: Vec<VisibleRichTextLine> = Vec::with_capacity(visible_count);
        for i in 0..visible_count {
            let line_idx = resolved_scroll_offset + i;
            let row_y = content_bounds.y + i as f32 * measure.row_height;
            visible_lines.push(VisibleRichTextLine {
                line_idx,
                bounds: Rect::new(
                    content_bounds.x,
                    row_y,
                    content_bounds.width,
                    measure.row_height,
                ),
            });
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
                    measure.row_height,
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
