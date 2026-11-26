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
//!
//! # Font role (editor vs chrome)
//!
//! This primitive is classified [`crate::ChromePrimitive::RichTextPopup`]
//! — its content paints in the chrome (UI) font unless the descriptor
//! says otherwise. [`RichTextPopup::font_role`] is that say: a consumer
//! rendering editor content (hover docs, signatures, diagnostics) sets
//! it to [`crate::FontRole::Editor`] so the body is measured, laid out
//! and link-hit-tested in the editor's monospace metrics, keeping the
//! hit regions in sync with the glyphs actually painted.
//!
//! The role lives on the descriptor rather than in the call, so the one
//! widget-contract path ([`Primitive::measure`], [`Primitive::layout`],
//! [`Primitive::font_role`]) covers both fonts. The field is
//! `#[serde(default)]` and [`crate::FontRole`]'s own `Default` is
//! [`crate::FontRole::Chrome`], so a frame deserialized without the
//! field keeps the chrome metrics.

use crate::event::{Key, MouseButton, NamedKey, Rect};
use crate::interactive::{
    EventCx, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget, WidgetAction,
};
use crate::layout::{Anchor, Side};
use crate::primitive::{LayoutCtx, Primitive, Size, Units};
use crate::primitives::scrollbar::{scroll_by_clamped, wheel_row_delta};
use crate::types::{Color, StyledText, WidgetId};
use crate::{FontRole, Handled};
use serde::{Deserialize, Serialize};

/// Width of the popup's frame, in the host's own units — one cell on a
/// terminal, one pixel on a GUI surface — on each of the four sides.
/// Shared by the box-sizing in [`RichTextPopup::layout`] and the
/// content-area inset in [`RichTextPopup::resolve_at`] so the two can
/// never disagree.
const BORDER: f32 = 1.0;

/// Declarative description of a rich-text popup.
///
/// # Examples
///
/// ```
/// use quadraui::{RichTextPopup, RichTextPopupHit, RichTextPopupMeasure, Rect, StyledText, WidgetId};
///
/// let popup = RichTextPopup {
///     id: WidgetId::new("popup:hover"),
///     lines: vec![StyledText::plain("fn main() -> ()")],
///     line_text: vec!["fn main() -> ()".to_string()],
///     line_scales: vec![],
///     scroll_top: 0,
///     max_visible_rows: 10,
///     has_focus: true,
///     selection: None,
///     links: vec![],
///     focused_link: None,
///     placement: Default::default(),
///     padding: 1.0,
///     fg: None,
///     bg: None,
///     font_role: Default::default(),
/// };
///
/// let viewport = Rect::new(0.0, 0.0, 80.0, 24.0);
/// let measure = RichTextPopupMeasure::new(40.0, 1.0);
/// let layout = popup.layout(5.0, 5.0, viewport, measure, |_, start, end| (end - start) as f32);
///
/// let inside = (layout.content_bounds.x + 1.0, layout.content_bounds.y + 0.5);
/// assert_eq!(layout.hit_test(inside.0, inside.1), RichTextPopupHit::Body);
/// assert_eq!(layout.char_at(inside.0, inside.1, 1.0), Some((0, 1)));
/// ```
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
    /// Which font the body is measured, laid out and link-hit-tested in
    /// (see this module's *Font role* section).
    /// [`FontRole::Chrome`] renders the popup as host chrome;
    /// [`FontRole::Editor`] gives it the editor's monospace metrics, for
    /// hover docs and signature help that sit inside editor text.
    #[serde(default)]
    pub font_role: FontRole,
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
        let max_rows = self.max_visible_rows.max(1);
        let (resolved_scroll_offset, visible_count) = self.scroll_window(max_rows);
        let content_h = self.content_height(measure, resolved_scroll_offset, visible_count);

        let pad = self.padding.max(0.0);
        let border = BORDER; // 1 cell / 1 pixel each side
        let outer_w = measure.content_width + pad * 2.0 + border * 2.0;
        let outer_h = content_h + pad * 2.0 + border * 2.0;

        // Horizontal position is clamped into the viewport *before*
        // `Anchor::resolve` runs, independently of the vertical flip
        // decision below. `Anchor::resolve`'s `fits` predicate tests
        // both axes together, so an anchor `x` that is already outside
        // the viewport would make the horizontal overflow fail the fit
        // test on whichever vertical side is tried, masking the
        // intended vertical flip.
        let ax = anchor_x.clamp(
            viewport.x,
            (viewport.x + viewport.width - outer_w).max(viewport.x),
        );

        // Placement: above the anchor when there's room, otherwise below,
        // via the shared `Anchor` flip/clamp resolver. The anchor rect's
        // width is set to `outer_w` so `Anchor::resolve`'s cross-axis
        // centering (`(rect.width - w) * 0.5`) collapses to zero and `x`
        // stays left-aligned with `ax`, matching this popup's left-aligned
        // (not centered) horizontal placement; its height is
        // `measure.row_height` so the `Below` candidate opens flush under
        // the anchor cell, same as the `Above` candidate opens flush above
        // it.
        let side = match self.placement {
            PopupPlacement::Above => Side::Top,
            PopupPlacement::Below => Side::Bottom,
        };
        let anchor_rect = Rect::new(ax, anchor_y, outer_w, measure.row_height);
        let (origin, _resolved_side) =
            Anchor::new(anchor_rect, side).resolve((outer_w, outer_h), viewport);
        let bounds = Rect::new(origin.x, origin.y, outer_w, outer_h);

        self.resolve_at(bounds, max_rows, measure, link_widths)
    }

    /// The row measure the widget contract resolves rows with: the
    /// backend's own text line height (not the chrome row floor) and its
    /// per-row scaling capability — the measure a host passes
    /// [`Self::layout`] when it follows [`RichTextPopupMeasure`]'s
    /// convention.
    fn contract_measure(ctx: &LayoutCtx, content_width: f32) -> RichTextPopupMeasure {
        let line_height = ctx.editor_line_height();
        let row_height = if line_height > 0.0 { line_height } else { 1.0 };
        RichTextPopupMeasure::new(content_width, row_height).with_scale_rows(ctx.scales_text_rows())
    }

    /// The largest row window, capped by [`Self::max_visible_rows`] and
    /// never below one row, whose (possibly scaled) rows fit in `inner_h`.
    /// Each candidate is measured at the scroll offset that window
    /// itself resolves to, so the count agrees with
    /// [`Self::content_height`] for the rows actually shown.
    fn rows_that_fit(&self, measure: RichTextPopupMeasure, inner_h: f32) -> usize {
        // Absorbs the rounding of a box height that was itself summed
        // from these row heights.
        let slack = measure.row_height * 1e-3;
        let mut max_rows = self.max_visible_rows.max(1);
        while max_rows > 1 {
            let (offset, count) = self.scroll_window(max_rows);
            if self.content_height(measure, offset, count) <= inner_h + slack {
                break;
            }
            max_rows -= 1;
        }
        max_rows
    }

    /// The visible window for a `max_rows`-row viewport: the scroll
    /// offset clamped so a stale [`Self::scroll_top`] still lands on the
    /// last full screen, and how many lines that window shows.
    fn scroll_window(&self, max_rows: usize) -> (usize, usize) {
        let total_lines = self.lines.len();
        // Clamp scroll FIRST so a stale `scroll_top` past `max_scroll`
        // still produces a valid visible window (last full screen).
        let max_scroll = total_lines.saturating_sub(max_rows);
        let resolved_scroll_offset = self.scroll_top.min(max_scroll);
        let visible_count = total_lines
            .saturating_sub(resolved_scroll_offset)
            .min(max_rows);
        (resolved_scroll_offset, visible_count)
    }

    /// Height of one row. Fixed-cell backends (`scale_rows == false`,
    /// e.g. TUI) keep every row at `measure.row_height`; scaling backends
    /// (GTK) reserve `row_height * line_scales[i]` so larger heading
    /// glyphs don't overlap the rows below. Scales below 1.0 are clamped
    /// so a stray small scale can't shrink a row.
    fn row_height_at(&self, measure: RichTextPopupMeasure, line_idx: usize) -> f32 {
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
    }

    /// Total content height = sum of the visible window's row heights
    /// (varies with which rows are scrolled into view when scales differ).
    fn content_height(
        &self,
        measure: RichTextPopupMeasure,
        resolved_scroll_offset: usize,
        visible_count: usize,
    ) -> f32 {
        (0..visible_count)
            .map(|i| self.row_height_at(measure, resolved_scroll_offset + i))
            .sum()
    }

    /// Resolve the content area, visible rows, scrollbar and link hit
    /// zones for a popup box already placed at `bounds`.
    ///
    /// The shared core of [`Self::layout`] — which picks `bounds` against
    /// an anchor first — and
    /// [`Primitive::layout`], which
    /// is handed the already-placed box by its caller.
    fn resolve_at<W>(
        &self,
        bounds: Rect,
        max_rows: usize,
        measure: RichTextPopupMeasure,
        link_widths: W,
    ) -> RichTextPopupLayout
    where
        W: Fn(usize, usize, usize) -> f32,
    {
        let total_lines = self.lines.len();
        let (resolved_scroll_offset, visible_count) = self.scroll_window(max_rows);
        let content_h = self.content_height(measure, resolved_scroll_offset, visible_count);
        let pad = self.padding.max(0.0);
        let border = BORDER;

        let content_bounds = Rect::new(
            bounds.x + border + pad,
            bounds.y + border + pad,
            measure.content_width,
            content_h,
        );

        // Visible lines — accumulate `y` by each row's (possibly scaled)
        // height so rows never overlap on scaling backends.
        let mut visible_lines: Vec<VisibleRichTextLine> = Vec::with_capacity(visible_count);
        let mut row_y = content_bounds.y;
        for i in 0..visible_count {
            let line_idx = resolved_scroll_offset + i;
            let row_h = self.row_height_at(measure, line_idx);
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
            // Thumb geometry via the one canonical formula
            // instead of a hand-rolled restatement of it: scroll is
            // row-indexed here, so `scroll_rows -> main-axis units` uses
            // the same "row_main_unit" conversion `multi_section_view`'s
            // `compute_thumb_bounds` does, then `fit_thumb` sizes and
            // positions the thumb with `measure.row_height` as the
            // minimum thumb length (never smaller than one row).
            let row_main_unit = content_bounds.height / total_lines as f32;
            let (thumb_top_offset, thumb_h) = crate::primitives::scrollbar::fit_thumb(
                resolved_scroll_offset as f32 * row_main_unit,
                content_bounds.height,
                max_rows as f32 * row_main_unit,
                content_bounds.height,
                measure.row_height,
            );
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

// ── Widget contract: `Primitive` + `Interactive` ───────────────────────────

/// The semantic outcomes a [`RichTextPopup`] reports — its
/// [`Interactive::Action`].
///
/// [`RichTextPopup`] stays purely declarative: neither variant mutates
/// the popup. Each reports the state the host should write back onto its
/// own `RichTextPopup` before the next frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RichTextPopupEvent {
    /// A clickable [`RichTextLink`] was activated — the host opens
    /// `url`.
    LinkClicked { url: String },
    /// A wheel gesture or a Page key scrolled the body. The host writes
    /// `new_offset` onto [`RichTextPopup::scroll_top`].
    Scrolled { new_offset: usize },
}

impl WidgetAction for RichTextPopupEvent {
    /// Both outcomes have an exact [`crate::UiEvent`] of their own:
    /// [`crate::UiEvent::RichTextLinkClicked`] for a link, and the
    /// cross-primitive [`crate::UiEvent::ScrollOffsetChanged`] for the
    /// body offset — the same generic carrier every other scrollable
    /// primitive's offset rides.
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            RichTextPopupEvent::LinkClicked { url } => crate::UiEvent::RichTextLinkClicked(id, url),
            RichTextPopupEvent::Scrolled { new_offset } => crate::UiEvent::ScrollOffsetChanged {
                widget: id,
                new_offset,
            },
        }
    }
}

/// [`RichTextPopup`]'s fully-resolved widget-contract layout: the
/// absolute `area` the popup was laid out in, the row pitch it was
/// resolved against, how many rows that box shows, and the
/// [`RichTextPopupLayout`] itself.
///
/// `#[non_exhaustive]`: produced by
/// [`Primitive::layout`], never
/// built by a consumer via struct literal — same convention
/// [`RichTextPopupLayout`] itself follows.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RichTextPopupPrimitiveLayout {
    /// The rect [`Primitive::layout`]
    /// was called with — the popup's whole box, already placed by the
    /// host.
    pub area: Rect,
    /// Unscaled row pitch the rows were resolved at, from
    /// [`crate::LayoutCtx::editor_line_height`].
    pub row_height: f32,
    /// Advance of one character in the font the rows are measured in,
    /// from [`crate::LayoutCtx::char_width`] —
    /// [`RichTextPopupLayout::char_at`]'s `col_width`.
    pub char_width: f32,
    /// How many body rows the box shows at once — the page size a Page
    /// key scrolls by, and the window [`Self::max_scroll`] measures
    /// against.
    pub visible_rows: usize,
    /// The resolved popup layout. Its
    /// `bounds`/`content_bounds`/`visible_lines`/`scrollbar`/
    /// `link_hit_regions` are in the same absolute space as
    /// [`Self::area`].
    pub layout: RichTextPopupLayout,
}

impl RichTextPopupPrimitiveLayout {
    /// Hit-test at a position relative to [`Self::area`]'s own origin —
    /// the space [`RoutedEvent::Pointer`]'s `local` field uses.
    pub fn hit_test_local(&self, x: f32, y: f32) -> RichTextPopupHit {
        self.layout.hit_test(self.area.x + x, self.area.y + y)
    }

    /// [`RichTextPopupLayout::char_at`] at a position relative to
    /// [`Self::area`]'s own origin, measured against the font this
    /// layout was resolved in.
    pub fn char_at_local(&self, x: f32, y: f32) -> Option<(usize, usize)> {
        self.layout
            .char_at(self.area.x + x, self.area.y + y, self.char_width)
    }

    /// The largest [`RichTextPopup::scroll_top`] that still fills the
    /// box — scrolling stops at the last full screen rather than running
    /// off the end into blank rows.
    pub fn max_scroll(&self, total_lines: usize) -> usize {
        total_lines.saturating_sub(self.visible_rows.max(1))
    }
}

impl Primitive for RichTextPopup {
    type Layout = RichTextPopupPrimitiveLayout;
    type Event = RichTextPopupEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::RichTextPopup(self.clone())
    }

    /// [`RichTextPopup::font_role`] — the descriptor decides, rather
    /// than the trait's chrome-only default, so an editor-role popup is
    /// measured and link-hit-tested in the editor font.
    fn font_role(&self) -> FontRole {
        self.font_role
    }

    /// The box that shows every line the popup is willing to show at
    /// once ([`RichTextPopup::max_visible_rows`], capped by the line
    /// count): the widest line's own measured width plus
    /// [`RichTextPopup::padding`] and the [`BORDER`] frame on each side.
    fn measure(&self, ctx: &LayoutCtx, _avail: Size) -> crate::NaturalSize {
        let cells = matches!(ctx.units(), Units::Cells);
        let width_of = |text: &str| -> f32 {
            if cells {
                crate::text_util::display_width(text) as f32
            } else {
                ctx.text_width(text, self.font_role())
            }
        };
        let content_w = self
            .lines
            .iter()
            .map(|line| line.spans.iter().map(|s| width_of(&s.text)).sum::<f32>())
            .fold(0.0_f32, f32::max);

        let max_rows = self.max_visible_rows.max(1);
        let measure = Self::contract_measure(ctx, content_w);
        let (offset, count) = self.scroll_window(max_rows);
        let content_h = self.content_height(measure, offset, count);

        let chrome = 2.0 * (BORDER + self.padding.max(0.0));
        crate::NaturalSize::new(Some(content_w + chrome), Some(content_h + chrome))
    }

    /// `rect` is the popup's final box — the `bounds` a host already
    /// resolved through [`RichTextPopup::layout`]'s anchored placement —
    /// so the content area, rows, scrollbar and link zones are resolved
    /// inside it rather than re-placed.
    ///
    /// Rows are measured the way that host sized the box: one
    /// [`LayoutCtx::editor_line_height`] (the backend's own
    /// [`crate::Backend::line_height`]) per unscaled row, each scaled by
    /// [`RichTextPopup::line_scales`] when
    /// [`LayoutCtx::scales_text_rows`] says the backend scales text. The
    /// row count is the largest window, capped by
    /// [`RichTextPopup::max_visible_rows`], whose rows fit inside the
    /// frame and padding — so a box [`RichTextPopup::layout`] sized shows
    /// every row it was sized for, and a shorter box shows what it can
    /// hold instead of overflowing.
    /// Link hit zones are measured with `ctx`'s own text measurer, so
    /// they line up with the glyphs the backend paints.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> RichTextPopupPrimitiveLayout {
        let cells = matches!(ctx.units(), Units::Cells);
        let chrome = 2.0 * (BORDER + self.padding.max(0.0));
        let measure = Self::contract_measure(ctx, (rect.width - chrome).max(0.0));
        let row_height = measure.row_height;
        let inner_h = (rect.height - chrome).max(0.0);
        let max_rows = self.rows_that_fit(measure, inner_h);

        let role = self.font_role();
        let link_widths = |line: usize, start: usize, end: usize| -> f32 {
            let Some(text) = self.line_text.get(line) else {
                return 0.0;
            };
            let slice = text.get(start..end).unwrap_or("");
            if cells {
                crate::text_util::display_width(slice) as f32
            } else {
                ctx.text_width(slice, role)
            }
        };

        let layout = self.resolve_at(rect, max_rows, measure, link_widths);
        RichTextPopupPrimitiveLayout {
            area: rect,
            row_height,
            char_width: ctx.char_width,
            visible_rows: max_rows,
            layout,
        }
    }

    /// The shared pixel painter — the same
    /// [`native_surface_paint::paint`] the macOS and Windows
    /// `draw_rich_text_popup` wrappers call. GTK's own wrapper keeps a
    /// Pango `AttrList` painter of its own (see that module's doc for
    /// why); this seam paints GTK through the shared per-span painter
    /// like every other pixel surface.
    fn paint(
        &self,
        layout: &RichTextPopupPrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        theme: &crate::theme::Theme,
        style: &crate::style::Style,
    ) {
        let _ = style;
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            // The painter's return value is the per-link hit rectangles
            // it measured; this seam hit-tests through
            // `layout.link_hit_regions`, which `Self::layout` already
            // measured with the same backend's text measurer.
            let _ = native_surface_paint::paint(self, &layout.layout, surface, theme);
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme);
        }
    }

    /// [`cell_paint::paint`] over `buf`, reached through
    /// [`crate::tui::draw_rich_text_popup`] because the `Buffer` →
    /// [`crate::CellSurface`] adapter is private to `crate::tui`.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &RichTextPopupPrimitiveLayout,
        buf: &mut ratatui::buffer::Buffer,
        theme: &crate::theme::Theme,
    ) {
        // The painter never moves the cursor, so the sink is never read.
        let mut cursor_sink = None;
        let mut surface = crate::tui::cell_surface::TuiCellSurface::new(
            buf,
            crate::backend::ColorDepth::TrueColor,
            &mut cursor_sink,
        );
        cell_paint::paint(self, &layout.layout, &mut surface, theme);
    }
}

impl RichTextPopup {
    /// Whether the body is taller than the box shows — the one condition
    /// that makes this popup keyboard-navigable on its own.
    fn overflows(&self, layout: &RichTextPopupPrimitiveLayout) -> bool {
        self.lines.len() > layout.visible_rows
    }

    /// Scroll the body by `delta` rows, clamped to the last full screen,
    /// and report the new offset — or [`Handled::Consumed`] when the
    /// gesture lands on an offset the popup already shows (it still owns
    /// the gesture, so an editor behind it never scrolls too).
    fn scroll_by(
        &self,
        delta: isize,
        layout: &RichTextPopupPrimitiveLayout,
    ) -> Handled<RichTextPopupEvent> {
        if !self.overflows(layout) {
            return Handled::Ignored;
        }
        let max = layout.max_scroll(self.lines.len());
        let current = self.scroll_top.min(max);
        let new_offset = scroll_by_clamped(current, delta, max);
        if new_offset == self.scroll_top {
            Handled::Consumed
        } else {
            Handled::Event(RichTextPopupEvent::Scrolled { new_offset })
        }
    }
}

impl Interactive for RichTextPopup {
    /// Nothing: the scroll offset lives on
    /// [`RichTextPopup::scroll_top`] and the focused link on
    /// [`RichTextPopup::focused_link`], so this widget has no
    /// interaction state the descriptor doesn't already carry.
    type State = ();
    type Action = RichTextPopupEvent;

    /// One focus stop as a whole whenever there is something to drive —
    /// a body that overflows its box (Page keys scroll it) or at least
    /// one clickable link. A short, link-free popup is pure content, so
    /// it stays out of the tab order.
    ///
    /// "Overflows" is measured against
    /// [`RichTextPopup::max_visible_rows`], the descriptor's own ask,
    /// because [`FocusSpec`] is resolved before any box exists;
    /// [`Primitive::layout`] caps the row count again by what the box it
    /// is given can actually hold. A link-free popup whose line count
    /// fits `max_visible_rows` but not a box shorter than that is
    /// therefore not a focus stop even though its body is clipped — the
    /// descriptor asked for a box that would have shown it all.
    fn focus(&self) -> FocusSpec {
        if self.links.is_empty() && self.lines.len() <= self.max_visible_rows.max(1) {
            FocusSpec::None
        } else {
            FocusSpec::Stop
        }
    }

    fn handle(
        &self,
        _state: &mut (),
        event: &RoutedEvent,
        layout: &RichTextPopupPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<RichTextPopupEvent> {
        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                local,
                ..
            } => match layout.hit_test_local(local.x, local.y) {
                RichTextPopupHit::Link(idx) => {
                    cx.request_focus(None);
                    match self.links.get(idx) {
                        Some(link) => Handled::Event(RichTextPopupEvent::LinkClicked {
                            url: link.url.clone(),
                        }),
                        None => Handled::Consumed,
                    }
                }
                // A click anywhere else inside the box (body text, a
                // character, the scrollbar) belongs to the popup: it
                // takes focus and swallows the click rather than letting
                // it reach the editor behind it. A click outside is not
                // the popup's business at all — the host's own
                // dismiss-on-outside-click policy owns that.
                RichTextPopupHit::Char(..)
                | RichTextPopupHit::Body
                | RichTextPopupHit::ScrollbarTrack
                | RichTextPopupHit::ScrollbarThumb => {
                    cx.request_focus(None);
                    Handled::Consumed
                }
                RichTextPopupHit::Outside => Handled::Ignored,
            },
            RoutedEvent::Wheel { dy, .. } => self.scroll_by(wheel_row_delta(*dy, 1.0), layout),
            RoutedEvent::Key(KeyEvent { key, .. }) => match key {
                Key::Named(NamedKey::PageDown) => {
                    self.scroll_by(layout.visible_rows.max(1) as isize, layout)
                }
                Key::Named(NamedKey::PageUp) => {
                    self.scroll_by(-(layout.visible_rows.max(1) as isize), layout)
                }
                // Escape is deliberately not claimed: dismissing the
                // popup is the host's policy, not the popup's, so the
                // keystroke must keep travelling.
                _ => Handled::Ignored,
            },
            _ => Handled::Ignored,
        }
    }

    /// One sub-part per clickable link, addressed by its index into
    /// [`RichTextPopup::links`], in [`Self::handle`]'s own local space.
    /// Body rows carry no sub-target: they are addressed by `(line,
    /// col)` through [`RichTextPopupPrimitiveLayout::char_at_local`],
    /// which a [`SubTarget`]'s single opaque `u64` cannot name without
    /// colliding with a link index.
    fn sub_regions(&self, layout: &RichTextPopupPrimitiveLayout) -> Vec<(Rect, SubTarget)> {
        layout
            .layout
            .link_hit_regions
            .iter()
            .map(|(rect, idx)| {
                (
                    Rect::new(
                        rect.x - layout.area.x,
                        rect.y - layout.area.y,
                        rect.width,
                        rect.height,
                    ),
                    SubTarget(*idx as u64),
                )
            })
            .collect()
    }
}

/// The cell-grid painter for [`RichTextPopup`], written against
/// [`crate::CellSurface`].
///
/// A closed box — `┌─┐`/`└─┘` and `│` sides — tinted
/// [`crate::Theme::link_fg`] when [`RichTextPopup::has_focus`], else
/// [`crate::Theme::hover_border`]. Inside it, each visible line's styled
/// spans paint cell by cell with the selection inverted (`fg`/`bg`
/// swapped), the focused link's characters underlined, and the row padded
/// out to the content width so a short line still carries the popup's own
/// background. A scrollbar replaces the right border column when the
/// content overflows: `█` over the thumb's rows, `░` over the rest of the
/// track.
///
/// Per-popup `popup.fg`/`popup.bg` overrides win over
/// [`crate::Theme::hover_fg`]/[`crate::Theme::hover_bg`].
/// [`RichTextPopup::line_scales`] is ignored — a terminal cannot change
/// cell size mid-render.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{RichTextPopup, RichTextPopupLayout};
    use crate::cell_surface::{BorderSet, CellStyle, CellSurface};
    use crate::theme::Theme;
    use crate::types::Color;
    use crate::Rect;

    fn style(fg: Color, bg: Color) -> CellStyle {
        CellStyle {
            fg: Some(fg),
            bg: Some(bg),
            ..CellStyle::default()
        }
    }

    /// Paint `popup` at its resolved `layout` onto `surface`.
    pub(crate) fn paint(
        popup: &RichTextPopup,
        layout: &RichTextPopupLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
    ) {
        let bx = layout.bounds.x.round() as u16;
        let by = layout.bounds.y.round() as u16;
        let bw = layout.bounds.width.round() as u16;
        let bh = layout.bounds.height.round() as u16;
        if bw == 0 || bh == 0 {
            return;
        }

        let bg = popup.bg.unwrap_or(theme.hover_bg);
        let fg = popup.fg.unwrap_or(theme.hover_fg);
        let border = if popup.has_focus {
            theme.link_fg
        } else {
            theme.hover_border
        };

        // Content wash first, then the frame on top of it, so the inner
        // rows start from the popup's own background.
        //
        // [`CellSurface::draw_border`] paints nothing below 2x2, so a
        // box that degenerate shows the wash alone. Both `layout` paths
        // size the box around at least one content row plus the frame on
        // each side, so 2x2 is the floor they can produce.
        surface.fill(
            Rect::new(bx as f32, by as f32, bw as f32, bh as f32),
            style(fg, bg),
        );
        surface.draw_border(
            Rect::new(bx as f32, by as f32, bw as f32, bh as f32),
            BorderSet::Square,
            style(border, bg),
        );

        for vis in &layout.visible_lines {
            let row_y = vis.bounds.y.round() as u16;
            let line_x = vis.bounds.x.round() as u16;
            let line_w = vis.bounds.width.round() as u16;
            if bh > 0 && row_y >= by + bh - 1 {
                break;
            }

            let line_idx = vis.line_idx;
            let focused_link_range = popup.focused_link.and_then(|fi| {
                popup
                    .links
                    .get(fi)
                    .filter(|l| l.line == line_idx)
                    .map(|l| (l.start_byte, l.end_byte))
            });

            let mut col_off: u16 = 0;
            let mut byte_pos: usize = 0;
            if let Some(styled) = popup.lines.get(line_idx) {
                for span in &styled.spans {
                    let span_fg = span.fg.unwrap_or(fg);
                    let span_bg = span.bg.unwrap_or(bg);
                    for ch in span.text.chars() {
                        if col_off >= line_w {
                            break;
                        }
                        let char_col = col_off as usize;
                        // Selection inverts the cell's own colours.
                        let in_selection = popup
                            .selection
                            .map(|s| s.contains(line_idx, char_col))
                            .unwrap_or(false);
                        let (cell_fg, cell_bg) = if in_selection {
                            (bg, span_fg)
                        } else {
                            (span_fg, span_bg)
                        };
                        let mut cell_style = style(cell_fg, cell_bg);
                        cell_style.bold = span.bold;
                        cell_style.italic = span.italic;
                        if popup.has_focus
                            && !in_selection
                            && focused_link_range
                                .map(|(s, e)| byte_pos >= s && byte_pos < e)
                                .unwrap_or(false)
                        {
                            cell_style.underline = Some(cell_fg);
                        }
                        surface.put_char(line_x + col_off, row_y, ch, cell_style);
                        col_off += 1;
                        byte_pos += ch.len_utf8();
                    }
                }
            }
            // Pad the rest of the line with the popup background, still
            // honouring a selection that runs past the line's own text.
            while col_off < line_w {
                let in_selection = popup
                    .selection
                    .map(|s| s.contains(line_idx, col_off as usize))
                    .unwrap_or(false);
                let cell_bg = if in_selection { fg } else { bg };
                surface.put_char(line_x + col_off, row_y, ' ', style(fg, cell_bg));
                col_off += 1;
            }
        }

        // Scrollbar thumb on the right border, when present.
        if let Some(sb) = layout.scrollbar {
            let track_x = sb.track.x.round() as u16;
            let track_y = sb.track.y.round() as u16;
            let track_h = sb.track.height.round() as u16;
            let thumb_y = sb.thumb.y.round() as u16;
            let thumb_h = sb.thumb.height.round() as u16;
            for r in 0..track_h {
                let cy = track_y + r;
                let in_thumb = cy >= thumb_y && cy < thumb_y + thumb_h;
                let ch = if in_thumb { '█' } else { '░' };
                let cell_fg = if in_thumb { border } else { fg };
                surface.put_char(track_x, cy, ch, style(cell_fg, bg));
            }
        }
    }
}

/// Visible width of the rich-text-popup scrollbar, shared by
/// [`native_surface_paint::paint`] and each backend's own hit-testing
/// constant of the same value
/// (`gtk`/`macos::rich_text_popup::RICH_TEXT_POPUP_SB_WIDTH`) — wider
/// than [`RichTextPopupLayout::scrollbar`]'s bare 1-unit track so the
/// bar is paint+click-friendly. Lives outside the `native_surface_paint`
/// cfg gate (unlike the rest of that module) so a GTK build — which
/// only needs this constant, not the whole shared painter — doesn't
/// have to compile the ~200-line `paint` function to reach it. Its own
/// gate is therefore the union of the rasterisers that read it, so a
/// TUI-only build (which has no rich-text-popup scrollbar to paint)
/// doesn't carry an unused constant.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) const SB_WIDTH: f32 = 8.0;
/// Inset between the scrollbar's right edge and the popup's own right
/// border. See [`SB_WIDTH`]'s doc for why this sits outside the
/// `native_surface_paint` gate and carries this one instead.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) const SB_INSET: f32 = 1.0;

// Shared painting implementation for every pixel surface — the painter
// `Primitive::paint` runs on GTK, macOS and Windows alike, and the one
// `macos::rich_text_popup`/`win::rich_text_popup` delegate to.
//
// `gtk::rich_text_popup::draw_rich_text_popup` keeps a painter of its own
// because it shapes entire lines with Pango's `AttrList` to avoid glyph-position
// drift from kerning/ligatures that occurs when spans are shaped and advanced
// separately, and derives its link rects from `index_to_pos` — neither of which
// `PaintSurface`'s per-run verbs can express. That wrapper also reaches into
// this file for the scrollbar geometry constants above (re-exported as
// `gtk::rich_text_popup::RICH_TEXT_POPUP_SB_WIDTH`/`_SB_INSET`, same derivation
// `macos::rich_text_popup` already uses), so its own paint stays in sync with
// macOS/Windows's scrollbar geometry without duplicating the constant.
//
// macOS and Windows both use per-span manual x-advance (the approach below),
// so consolidating them closes gaps between the implementations:
//
// - Selection background + inverted foreground colour.
// - Bold span styling.
// - Focused-link underline (whole-span approximation).
// - Content-area clipping to prevent overlong lines from bleeding past the border.
// - Scrollbar width + track opacity: wider bar with translucent track.
//
// Font scaling per row (via `RichTextPopup::line_scales`) is not implemented
// — it requires per-row font swaps that would need backend-specific capabilities.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) mod native_surface_paint {
    use super::{RichTextPopup, RichTextPopupLayout, TextSelection};
    // `pub(crate)` (not a plain `use`) so `native_surface_paint::SB_WIDTH`/
    // `::SB_INSET` stay reachable from `macos::rich_text_popup` and
    // `win::rich_text_popup`, which both read the constants through this
    // module path rather than `crate::primitives::rich_text_popup::` directly.
    pub(crate) use super::{SB_INSET, SB_WIDTH};
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;
    use crate::{Point, Rect};

    /// Translate a `TextSelection` (char columns) into the byte range
    /// this line contributes to the selection. Returns `(0, 0)` when
    /// the line is outside the selection. Ported verbatim from
    /// `gtk::rich_text_popup`'s (and `win::rich_text_popup`'s identical
    /// copy of the) private helper of the same name.
    fn selection_byte_range(
        sel: TextSelection,
        line_idx: usize,
        line_text: &str,
    ) -> (usize, usize) {
        if line_idx < sel.start_line || line_idx > sel.end_line {
            return (0, 0);
        }
        let (start_col, end_col) = if sel.start_line == sel.end_line {
            (sel.start_col, sel.end_col)
        } else if line_idx == sel.start_line {
            (sel.start_col, line_text.chars().count())
        } else if line_idx == sel.end_line {
            (0, sel.end_col)
        } else {
            (0, line_text.chars().count())
        };
        if end_col <= start_col {
            return (0, 0);
        }
        (
            crate::text_util::char_to_byte_idx(line_text, start_col),
            crate::text_util::char_to_byte_idx(line_text, end_col),
        )
    }

    /// Paint a [`RichTextPopup`] at its resolved `layout` onto
    /// `surface` — background, border (accent when `popup.has_focus`),
    /// per-visible-line styled spans (selection-bg + inverted fg,
    /// bold, focused-link underline), and the scrollbar when present.
    /// Returns the per-link hit rectangles `(Rect, url)` computed from
    /// `surface`'s own glyph measurements — more accurate than
    /// `layout.link_hit_regions`, whose widths come from the host's
    /// `link_widths` measure closure rather than this backend's actual
    /// glyph advances (mirrors `gtk::draw_rich_text_popup`'s
    /// `index_to_pos`-derived link rects for the same reason).
    ///
    /// See this module's doc for why `gtk::rich_text_popup` does not
    /// call this — GTK's own paint is not migrated.
    pub(crate) fn paint(
        popup: &RichTextPopup,
        layout: &RichTextPopupLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
    ) -> Vec<(Rect, String)> {
        let bounds = layout.bounds;
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return Vec::new();
        }

        let bg = popup.bg.unwrap_or(theme.hover_bg);
        let fg = popup.fg.unwrap_or(theme.hover_fg);
        let border = if popup.has_focus {
            theme.link_fg
        } else {
            theme.hover_border
        };

        surface.surface_fill_rect(bounds, bg);
        surface.surface_stroke_rect(bounds, border, 1.0);

        let content = layout.content_bounds;
        surface.surface_push_clip(content);

        let mut link_rects: Vec<(Rect, String)> = Vec::new();

        for vis in &layout.visible_lines {
            let line_idx = vis.line_idx;
            let raw_text = popup
                .line_text
                .get(line_idx)
                .map(String::as_str)
                .unwrap_or("");
            let Some(styled) = popup.lines.get(line_idx) else {
                continue;
            };

            let (sel_start, sel_end) = popup
                .selection
                .map(|sel| selection_byte_range(sel, line_idx, raw_text))
                .unwrap_or((0, 0));
            if sel_end > sel_start {
                // Selection bg is painted as a single rect spanning the
                // byte range's measured width, ahead of the text itself.
                let pre_w = surface.surface_measure_text(&raw_text[..sel_start]).0;
                let sel_w = surface
                    .surface_measure_text(&raw_text[sel_start..sel_end])
                    .0;
                let sel_rect = Rect::new(
                    vis.bounds.x + pre_w,
                    vis.bounds.y,
                    sel_w.max(1.0),
                    vis.bounds.height,
                );
                surface.surface_fill_rect(sel_rect, popup.fg.unwrap_or(theme.foreground));
            }

            let focused_underline_range = if popup.has_focus {
                popup.focused_link.and_then(|idx| {
                    popup
                        .links
                        .get(idx)
                        .filter(|link| link.line == line_idx)
                        .map(|link| (link.start_byte, link.end_byte))
                })
            } else {
                None
            };

            let mut byte_pos = 0usize;
            let mut x = vis.bounds.x;
            for span in &styled.spans {
                let start = byte_pos;
                let end = byte_pos + span.text.len();
                let in_selection = sel_end > sel_start && start >= sel_start && end <= sel_end;
                let color = if in_selection {
                    popup.bg.unwrap_or(theme.background)
                } else {
                    span.fg.unwrap_or(fg)
                };
                let (w, _) = surface.surface_measure_text_styled(&span.text, span.bold);
                let rect = Rect::new(x, vis.bounds.y, w.max(1.0), vis.bounds.height);
                surface.surface_draw_text_run_styled(
                    rect,
                    &span.text,
                    color,
                    span.bold,
                    span.italic,
                    false,
                    1.0,
                );

                // Underline the whole span when it overlaps the focused
                // link's byte range. Coarser than GTK's per-substring
                // underline (which uses Pango's `index_to_pos` to
                // underline exactly the link's own characters within a
                // mixed span), but avoids re-slicing `span.text` at
                // arbitrary byte offsets that aren't guaranteed to land
                // on this span's own char boundaries.
                if let Some((us, ue)) = focused_underline_range {
                    if start < ue && end > us {
                        let uy = vis.bounds.y + vis.bounds.height - 2.0;
                        surface.surface_draw_line(
                            Point::new(x, uy),
                            Point::new(x + w, uy),
                            border,
                            1.0,
                        );
                    }
                }

                x += w;
                byte_pos = end;
            }

            for link in popup.links.iter().filter(|l| l.line == line_idx) {
                let pre_w = surface.surface_measure_text(&raw_text[..link.start_byte]).0;
                let span_w = surface
                    .surface_measure_text(&raw_text[link.start_byte..link.end_byte])
                    .0;
                let rect = Rect::new(
                    vis.bounds.x + pre_w,
                    vis.bounds.y,
                    span_w.max(1.0),
                    vis.bounds.height,
                );
                link_rects.push((rect, link.url.clone()));
            }
        }

        surface.surface_pop_clip();

        if let Some(sb) = layout.scrollbar {
            let sb_x = bounds.x + bounds.width - SB_WIDTH - SB_INSET;
            let track = Rect::new(sb_x, sb.track.y, SB_WIDTH, sb.track.height);
            surface.surface_fill_rect(track, theme.muted_fg.with_alpha(0.3));
            let thumb_top_off = sb.thumb.y - sb.track.y;
            let thumb = Rect::new(
                sb_x + 1.0,
                sb.track.y + thumb_top_off,
                SB_WIDTH - 2.0,
                sb.thumb.height,
            );
            surface.surface_fill_rect(thumb, border);
        }

        link_rects
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
            font_role: FontRole::Chrome,
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

    // ── RichTextPopup primitive tests ────────────────────────────────────

    fn make_rich_popup(lines: usize, max_visible: usize, scroll: usize) -> RichTextPopup {
        let line_text: Vec<String> = (0..lines).map(|i| format!("line {i:02}")).collect();
        let lines_styled: Vec<StyledText> = line_text
            .iter()
            .map(|s| StyledText::plain(s.clone()))
            .collect();
        RichTextPopup {
            id: WidgetId::new("hover"),
            lines: lines_styled,
            line_text,
            line_scales: Vec::new(),
            scroll_top: scroll,
            max_visible_rows: max_visible,
            has_focus: false,
            selection: None,
            links: Vec::new(),
            focused_link: None,
            placement: PopupPlacement::Above,
            padding: 1.0,
            fg: None,
            bg: None,
            font_role: FontRole::Chrome,
        }
    }

    #[test]
    fn rich_text_popup_layout_visible_lines_window() {
        let p = make_rich_popup(30, 10, 5);
        let viewport = Rect::new(0.0, 0.0, 200.0, 200.0);
        let layout = p.layout(
            10.0,
            100.0,
            viewport,
            RichTextPopupMeasure::new(80.0, 1.0),
            |_, s, e| (e - s) as f32,
        );
        // Visible window starts at scroll_top=5 and shows 10 rows (capped by total).
        assert_eq!(layout.visible_lines.len(), 10);
        assert_eq!(layout.visible_lines[0].line_idx, 5);
        assert_eq!(layout.visible_lines[9].line_idx, 14);
        assert_eq!(layout.resolved_scroll_offset, 5);
    }

    #[test]
    fn rich_text_popup_layout_clamps_scroll_past_end() {
        // 30 lines, 10 visible at a time → max scroll is 20.
        // Asking for scroll=999 should clamp.
        let p = make_rich_popup(30, 10, 999);
        let viewport = Rect::new(0.0, 0.0, 200.0, 200.0);
        let layout = p.layout(
            0.0,
            0.0,
            viewport,
            RichTextPopupMeasure::new(80.0, 1.0),
            |_, s, e| (e - s) as f32,
        );
        assert_eq!(layout.resolved_scroll_offset, 20);
        assert_eq!(layout.visible_lines.first().unwrap().line_idx, 20);
        assert_eq!(layout.visible_lines.last().unwrap().line_idx, 29);
    }

    #[test]
    fn rich_text_popup_scrollbar_present_when_overflow() {
        let p = make_rich_popup(50, 10, 0);
        let viewport = Rect::new(0.0, 0.0, 200.0, 200.0);
        let layout = p.layout(
            0.0,
            0.0,
            viewport,
            RichTextPopupMeasure::new(80.0, 1.0),
            |_, s, e| (e - s) as f32,
        );
        let sb = layout.scrollbar.expect("scrollbar should exist");
        // Thumb size proportional to visible/total = 10/50 = 1/5 of track.
        assert!(sb.thumb.height > 0.0);
        assert!(sb.thumb.height < sb.track.height);
        // No scrollbar when content fits.
        let p2 = make_rich_popup(5, 10, 0);
        let layout2 = p2.layout(
            0.0,
            0.0,
            viewport,
            RichTextPopupMeasure::new(80.0, 1.0),
            |_, s, e| (e - s) as f32,
        );
        assert!(layout2.scrollbar.is_none());
    }

    #[test]
    fn rich_text_popup_link_hit_regions_for_visible_lines() {
        let mut p = make_rich_popup(20, 10, 5);
        // Add a link on visible line 7 (visible_lines[2]).
        p.links.push(RichTextLink {
            line: 7,
            start_byte: 5,
            end_byte: 10,
            url: "https://example.com".to_string(),
        });
        // Add a link on hidden line 2 (above scroll_top=5).
        p.links.push(RichTextLink {
            line: 2,
            start_byte: 0,
            end_byte: 4,
            url: "off-screen".to_string(),
        });
        let viewport = Rect::new(0.0, 0.0, 200.0, 200.0);
        let layout = p.layout(
            10.0,
            100.0,
            viewport,
            RichTextPopupMeasure::new(80.0, 1.0),
            |_, s, e| (e - s) as f32,
        );
        // Only the visible link gets a hit region.
        assert_eq!(layout.link_hit_regions.len(), 1);
        let (_, idx) = &layout.link_hit_regions[0];
        assert_eq!(*idx, 0);
    }

    #[test]
    fn text_selection_contains_single_and_multi_line() {
        let single = TextSelection {
            start_line: 3,
            start_col: 5,
            end_line: 3,
            end_col: 10,
        };
        assert!(single.contains(3, 5));
        assert!(single.contains(3, 9));
        assert!(!single.contains(3, 10));
        assert!(!single.contains(2, 5));
        assert!(!single.contains(4, 5));

        let multi = TextSelection {
            start_line: 2,
            start_col: 4,
            end_line: 5,
            end_col: 3,
        };
        assert!(!multi.contains(2, 3));
        assert!(multi.contains(2, 4));
        assert!(multi.contains(3, 0));
        assert!(multi.contains(4, 100));
        assert!(multi.contains(5, 0));
        assert!(multi.contains(5, 2));
        assert!(!multi.contains(5, 3));
        assert!(!multi.contains(6, 0));
    }

    // ── Placement flip/clamp (via `crate::layout::Anchor`) ──────────────

    #[test]
    fn rich_text_popup_layout_flips_above_to_below_on_overflow() {
        // Preferred `Above`, but the anchor sits too close to the
        // viewport's top edge for the popup to fit above it.
        let p = make_rich_popup(3, 3, 0);
        let viewport = Rect::new(0.0, 0.0, 200.0, 200.0);
        let layout = p.layout(
            50.0,
            5.0,
            viewport,
            RichTextPopupMeasure::new(80.0, 10.0),
            |_, s, e| (e - s) as f32,
        );
        // outer_h = 3 rows * 10 + 2*padding(1) + 2*border(1) = 34; anchor_y
        // - outer_h would be negative, so it flips below the anchor cell.
        assert_eq!(layout.bounds.y, 15.0);
    }

    #[test]
    fn rich_text_popup_layout_flips_below_to_above_on_overflow() {
        // Preferred `Below`, but the anchor sits too close to the
        // viewport's bottom edge for the popup to fit below it.
        let mut p = make_rich_popup(3, 3, 0);
        p.placement = PopupPlacement::Below;
        let viewport = Rect::new(0.0, 0.0, 200.0, 200.0);
        let layout = p.layout(
            50.0,
            190.0,
            viewport,
            RichTextPopupMeasure::new(80.0, 10.0),
            |_, s, e| (e - s) as f32,
        );
        assert_eq!(layout.bounds.y, 156.0);
    }

    #[test]
    fn rich_text_popup_layout_pins_to_preferred_side_when_neither_fits() {
        // Viewport too short (40) for the 34-tall popup to fit fully on
        // either side of an anchor in the middle (y=20).
        //
        // When neither side fits, the popup pins to the preferred side
        // (matching `Tooltip`/`Float`/`ContextMenu`) and clamps, landing
        // at y=0.
        let p = make_rich_popup(3, 3, 0);
        let viewport = Rect::new(0.0, 0.0, 200.0, 40.0);
        let layout = p.layout(
            50.0,
            20.0,
            viewport,
            RichTextPopupMeasure::new(80.0, 10.0),
            |_, s, e| (e - s) as f32,
        );
        assert_eq!(layout.bounds.y, 0.0);
    }

    #[test]
    fn rich_text_popup_layout_clamps_x_to_viewport_when_anchor_near_right_edge() {
        let p = make_rich_popup(3, 3, 0);
        let viewport = Rect::new(0.0, 0.0, 200.0, 200.0);
        let layout = p.layout(
            190.0,
            50.0,
            viewport,
            RichTextPopupMeasure::new(80.0, 10.0),
            |_, s, e| (e - s) as f32,
        );
        // outer_w = 84; anchor_x + outer_w would overflow the 200-wide
        // viewport, so x is clamped to the viewport's right edge.
        assert_eq!(layout.bounds.x, 116.0);
    }

    #[test]
    fn rich_text_popup_layout_flips_above_when_anchor_overflows_both_axes() {
        // Anchor in the bottom-right corner: `anchor_x` (190) overflows
        // the viewport horizontally (outer_w 84 in a 200-wide viewport)
        // AND the preferred `Below` candidate overflows vertically
        // (anchor near the bottom). The vertical flip to `Above` must
        // still happen — horizontal overflow is clamped independently
        // and must not suppress it.
        let mut p = make_rich_popup(3, 3, 0);
        p.placement = PopupPlacement::Below;
        let viewport = Rect::new(0.0, 0.0, 200.0, 200.0);
        let layout = p.layout(
            190.0,
            190.0,
            viewport,
            RichTextPopupMeasure::new(80.0, 10.0),
            |_, s, e| (e - s) as f32,
        );
        // x clamps to the right edge: 200 - 84 = 116.
        assert_eq!(layout.bounds.x, 116.0);
        // Flips above: y = anchor_y - outer_h = 190 - 34 = 156.
        assert_eq!(layout.bounds.y, 156.0);
    }

    // ── Cell-path paint (`cell_paint::paint`) ───────────────────────────

    #[cfg(feature = "tui")]
    mod cells {
        use super::super::cell_test_paint::draw_rich_text_popup;
        use super::*;
        use crate::theme::Theme;
        use crate::tui::ratatui_color;
        use crate::types::{Color, StyledSpan};
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect as RRect;

        fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
            buf[(x, y)].symbol().chars().next().unwrap_or(' ')
        }

        fn cell_at(buf: &Buffer, x: u16, y: u16) -> &ratatui::buffer::Cell {
            &buf[(x, y)]
        }

        /// A popup whose box lands at `(0, 0)`: `PopupPlacement::Below`
        /// with the anchor one row above the viewport's top edge, so the
        /// tests below can index `buf` directly by row.
        fn layout_at_origin(p: &RichTextPopup, w: f32, h: f32) -> RichTextPopupLayout {
            let viewport = Rect::new(0.0, 0.0, w, h);
            p.layout(
                0.0,
                -1.0,
                viewport,
                RichTextPopupMeasure::new(w - 2.0 - 2.0 * p.padding.max(0.0), 1.0),
                |_, s, e| (e - s) as f32,
            )
        }

        fn popup(lines: &[&str]) -> RichTextPopup {
            RichTextPopup {
                id: WidgetId::new("rtp:cells"),
                lines: lines.iter().map(|l| StyledText::plain(*l)).collect(),
                line_text: lines.iter().map(|l| (*l).to_string()).collect(),
                line_scales: Vec::new(),
                scroll_top: 0,
                max_visible_rows: 10,
                has_focus: false,
                selection: None,
                links: Vec::new(),
                focused_link: None,
                placement: PopupPlacement::Below,
                padding: 0.0,
                fg: None,
                bg: None,
                font_role: FontRole::Chrome,
            }
        }

        #[test]
        fn paints_a_closed_box_around_the_content() {
            let p = popup(&["hello"]);
            let l = layout_at_origin(&p, 12.0, 10.0);
            let mut buf = Buffer::empty(RRect::new(0, 0, 20, 10));
            draw_rich_text_popup(&mut buf, &p, &l, &Theme::default());

            let w = l.bounds.width.round() as u16;
            let h = l.bounds.height.round() as u16;
            assert_eq!(cell_char(&buf, 0, 0), '┌');
            assert_eq!(cell_char(&buf, w - 1, 0), '┐');
            assert_eq!(cell_char(&buf, 0, h - 1), '└');
            assert_eq!(cell_char(&buf, w - 1, h - 1), '┘');
            assert_eq!(cell_char(&buf, 1, 0), '─');
            assert_eq!(cell_char(&buf, 0, 1), '│');
        }

        #[test]
        fn paints_each_visible_line_inside_the_border() {
            let p = popup(&["alpha", "beta"]);
            let l = layout_at_origin(&p, 12.0, 10.0);
            let mut buf = Buffer::empty(RRect::new(0, 0, 20, 10));
            draw_rich_text_popup(&mut buf, &p, &l, &Theme::default());

            let row0: String = (1..6).map(|x| cell_char(&buf, x, 1)).collect();
            let row1: String = (1..5).map(|x| cell_char(&buf, x, 2)).collect();
            assert_eq!(row0, "alpha");
            assert_eq!(row1, "beta");
        }

        /// A focused popup tints its frame with `link_fg`; an unfocused
        /// one uses `hover_border`.
        #[test]
        fn focus_tints_the_border() {
            let theme = Theme::default();
            let mut p = popup(&["hi"]);
            let l = layout_at_origin(&p, 10.0, 10.0);
            let mut buf = Buffer::empty(RRect::new(0, 0, 20, 10));
            draw_rich_text_popup(&mut buf, &p, &l, &theme);
            assert_eq!(cell_at(&buf, 0, 0).fg, ratatui_color(theme.hover_border));

            p.has_focus = true;
            let mut buf = Buffer::empty(RRect::new(0, 0, 20, 10));
            draw_rich_text_popup(&mut buf, &p, &l, &theme);
            assert_eq!(cell_at(&buf, 0, 0).fg, ratatui_color(theme.link_fg));
        }

        #[test]
        fn a_selected_character_inverts_its_colours() {
            let theme = Theme::default();
            let mut p = popup(&["abcdef"]);
            p.selection = Some(TextSelection {
                start_line: 0,
                start_col: 1,
                end_line: 0,
                end_col: 3,
            });
            let l = layout_at_origin(&p, 12.0, 10.0);
            let mut buf = Buffer::empty(RRect::new(0, 0, 20, 10));
            draw_rich_text_popup(&mut buf, &p, &l, &theme);

            // Columns 1..3 of the content row are selected: fg/bg swap.
            let selected = cell_at(&buf, 2, 1);
            assert_eq!(selected.fg, ratatui_color(theme.hover_bg));
            assert_eq!(selected.bg, ratatui_color(theme.hover_fg));
            let plain = cell_at(&buf, 4, 1);
            assert_eq!(plain.fg, ratatui_color(theme.hover_fg));
            assert_eq!(plain.bg, ratatui_color(theme.hover_bg));
        }

        #[test]
        fn the_focused_links_characters_are_underlined() {
            let mut p = popup(&["see docs"]);
            p.has_focus = true;
            p.links = vec![RichTextLink {
                line: 0,
                start_byte: 4,
                end_byte: 8,
                url: "https://example.com".into(),
            }];
            p.focused_link = Some(0);
            let l = layout_at_origin(&p, 14.0, 10.0);
            let mut buf = Buffer::empty(RRect::new(0, 0, 20, 10));
            draw_rich_text_popup(&mut buf, &p, &l, &Theme::default());

            let underlined = |x: u16| {
                cell_at(&buf, x, 1)
                    .modifier
                    .contains(ratatui::style::Modifier::UNDERLINED)
            };
            assert!(underlined(5), "'d' of \"docs\" must be underlined");
            assert!(!underlined(1), "'s' of \"see\" must not be");
        }

        #[test]
        fn an_overflowing_body_paints_a_thumb_scrollbar() {
            let lines: Vec<String> = (0..20).map(|i| format!("line{i}")).collect();
            let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
            let mut p = popup(&refs);
            p.max_visible_rows = 4;
            let l = layout_at_origin(&p, 12.0, 20.0);
            let mut buf = Buffer::empty(RRect::new(0, 0, 20, 20));
            draw_rich_text_popup(&mut buf, &p, &l, &Theme::default());

            let sb = l.scrollbar.expect("overflowing body scrolls");
            let track_x = sb.track.x.round() as u16;
            let painted: Vec<char> = (0..sb.track.height.round() as u16)
                .map(|r| cell_char(&buf, track_x, sb.track.y.round() as u16 + r))
                .collect();
            assert!(painted.contains(&'█'), "thumb glyph: {painted:?}");
            assert!(painted.contains(&'░'), "track glyph: {painted:?}");
        }

        #[test]
        fn a_per_span_foreground_override_wins_over_the_theme() {
            let mut p = popup(&["ab"]);
            p.lines = vec![StyledText {
                spans: vec![
                    StyledSpan {
                        fg: Some(Color::rgb(10, 200, 30)),
                        ..StyledSpan::plain("a")
                    },
                    StyledSpan::plain("b"),
                ],
            }];
            let l = layout_at_origin(&p, 10.0, 10.0);
            let mut buf = Buffer::empty(RRect::new(0, 0, 20, 10));
            draw_rich_text_popup(&mut buf, &p, &l, &Theme::default());
            assert_eq!(
                cell_at(&buf, 1, 1).fg,
                ratatui_color(Color::rgb(10, 200, 30))
            );
        }
    }

    // ── Widget contract: `Primitive` + `Interactive` ────────────────────

    mod contract {
        use super::*;
        use crate::interactive::{EventHost, FocusSpec, PointerKind, SubTarget};
        use crate::primitive::{LayoutCtx, Primitive, Size, Units};
        use crate::types::Modifiers;
        use crate::{Point, PointerShape};

        #[derive(Debug, Default)]
        struct MockHost {
            focus_requests: Vec<Option<SubTarget>>,
        }

        impl EventHost for MockHost {
            fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
                false
            }
            fn request_focus(&mut self, _id: &WidgetId, sub: Option<SubTarget>) {
                self.focus_requests.push(sub);
            }
            fn capture_pointer(&mut self, _id: &WidgetId) {}
            fn release_pointer(&mut self) {}
            fn set_cursor(&mut self, _cursor: PointerShape) {}
            fn request_redraw(&mut self) {}
            fn emit(&mut self, _event: crate::UiEvent) {}
            fn units(&self) -> Units {
                Units::Cells
            }
        }

        fn ctx() -> LayoutCtx<'static> {
            LayoutCtx::new(1.0, 1.0, false)
        }

        fn popup(lines: usize, max_visible: usize, scroll: usize) -> RichTextPopup {
            let mut p = make_rich_popup(lines, max_visible, scroll);
            p.padding = 0.0;
            p
        }

        fn layout_for(p: &RichTextPopup, rect: Rect) -> RichTextPopupPrimitiveLayout {
            Primitive::layout(p, &ctx(), rect)
        }

        fn handle(
            p: &RichTextPopup,
            event: &RoutedEvent,
            layout: &RichTextPopupPrimitiveLayout,
        ) -> (Handled<RichTextPopupEvent>, MockHost) {
            let mut host = MockHost::default();
            let mut cx = EventCx::new(p.id, &mut host);
            let out = Interactive::handle(p, &mut (), event, layout, &mut cx);
            (out, host)
        }

        fn key(named: NamedKey) -> RoutedEvent {
            RoutedEvent::Key(KeyEvent {
                key: Key::Named(named),
                modifiers: Modifiers::default(),
                repeat: false,
            })
        }

        fn down_at(x: f32, y: f32) -> RoutedEvent {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                sub: None,
                local: Point::new(x, y),
                mods: Modifiers::default(),
            }
        }

        #[test]
        fn layout_resolves_inside_the_host_placed_box() {
            let p = popup(10, 4, 0);
            let layout = layout_for(&p, Rect::new(3.0, 5.0, 20.0, 6.0));
            assert_eq!(layout.area, Rect::new(3.0, 5.0, 20.0, 6.0));
            assert_eq!(layout.layout.bounds, layout.area);
            // One border row/column of chrome on each side.
            assert_eq!(layout.layout.content_bounds.x, 4.0);
            assert_eq!(layout.layout.content_bounds.y, 6.0);
            assert_eq!(layout.layout.content_bounds.width, 18.0);
            assert_eq!(layout.visible_rows, 4);
            assert_eq!(layout.layout.visible_lines.len(), 4);
        }

        /// A box shorter than `max_visible_rows` shows only the rows it
        /// can hold rather than overflowing its own frame.
        #[test]
        fn a_short_box_caps_the_row_count() {
            let p = popup(10, 8, 0);
            let layout = layout_for(&p, Rect::new(0.0, 0.0, 20.0, 5.0));
            assert_eq!(layout.visible_rows, 3);
            assert_eq!(layout.layout.visible_lines.len(), 3);
        }

        /// A pixel host that sizes the box through [`RichTextPopup::layout`]
        /// at the backend's text line height, with scaled heading rows,
        /// gets back every row it sized for — the chrome row floor in
        /// `ctx.line_height` and the heading scales both resolve the
        /// same way on both sides.
        #[test]
        fn a_host_sized_box_with_scaled_rows_shows_every_sized_row() {
            let mut p = popup(12, 10, 0);
            p.padding = 1.0;
            p.line_scales = vec![1.6, 1.3, 1.1];
            let text_lh = 17.07;
            let measure = RichTextPopupMeasure::new(300.0, text_lh).with_scale_rows(true);
            let placed = p.layout(
                10.0,
                20.0,
                Rect::new(0.0, 0.0, 1000.0, 700.0),
                measure,
                |_, _, _| 0.0,
            );
            let ctx = LayoutCtx::new(22.0, 8.0, false)
                .with_editor_line_height(text_lh)
                .with_scaled_text_rows(true);
            let layout = Primitive::layout(&p, &ctx, placed.bounds);
            assert_eq!(layout.visible_rows, 10);
            assert_eq!(layout.layout.visible_lines, placed.visible_lines);
            assert_eq!(layout.layout.content_bounds, placed.content_bounds);
        }

        /// Scaled rows count against the box: a box holding ten unscaled
        /// rows holds fewer once the first rows are reserved taller.
        #[test]
        fn scaled_rows_reduce_the_row_count_a_box_can_hold() {
            let mut p = popup(12, 10, 0);
            p.line_scales = vec![2.0, 2.0];
            let ctx = LayoutCtx::new(10.0, 8.0, false).with_scaled_text_rows(true);
            let layout = Primitive::layout(&p, &ctx, Rect::new(0.0, 0.0, 200.0, 102.0));
            assert_eq!(layout.visible_rows, 8);
            let last = layout.layout.visible_lines.last().unwrap();
            assert!(last.bounds.y + last.bounds.height <= 101.0);
        }

        #[test]
        fn measure_reports_the_widest_line_plus_chrome() {
            let p = popup(3, 8, 0);
            let natural = Primitive::measure(&p, &ctx(), Size::new(80.0, 24.0));
            // "line 00" is 7 cells wide, plus one border column each side.
            assert_eq!(natural.width, Some(9.0));
            assert_eq!(natural.height, Some(5.0));
        }

        /// A pixel-unit measurer that gives the two font roles different
        /// advances, so a role-sensitive measurement is observable.
        fn role_measurer(text: &str, role: FontRole) -> f32 {
            let advance = if role == FontRole::Editor { 10.0 } else { 6.0 };
            text.chars().count() as f32 * advance
        }

        /// The descriptor's `font_role` is what [`Primitive::font_role`]
        /// reports, and it is the role `measure` and `layout` measure
        /// through — so an editor-role popup is sized and link-hit-tested
        /// in the editor font rather than always in chrome.
        #[test]
        fn the_descriptor_font_role_drives_measure_and_link_hit_regions() {
            let measurer = role_measurer;
            let base = LayoutCtx::new(16.0, 6.0, false);
            let ctx = base.with_measurer(&measurer);

            let mut chrome = popup(3, 8, 0);
            chrome.links.push(RichTextLink {
                line: 0,
                start_byte: 0,
                end_byte: 4,
                url: "https://example.com".into(),
            });
            let mut editor = chrome.clone();
            editor.font_role = FontRole::Editor;

            assert_eq!(Primitive::font_role(&chrome), FontRole::Chrome);
            assert_eq!(Primitive::font_role(&editor), FontRole::Editor);

            // "line 00" is 7 chars: 42px chrome / 70px editor, plus the
            // one-pixel border on each side (padding is 0 here).
            let width_of = |p: &RichTextPopup| {
                Primitive::measure(p, &ctx, Size::new(400.0, 200.0))
                    .width
                    .expect("popup measures its own width")
            };
            assert_eq!(width_of(&chrome), 44.0);
            assert_eq!(width_of(&editor), 72.0);

            // The link's hit region spans `line 00`'s first four chars,
            // measured in the popup's own role.
            let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
            let link_width = |p: &RichTextPopup| {
                Primitive::layout(p, &ctx, rect).layout.link_hit_regions[0]
                    .0
                    .width
            };
            assert_eq!(link_width(&chrome), 24.0);
            assert_eq!(link_width(&editor), 40.0);
        }

        #[test]
        fn a_link_free_short_popup_is_not_a_focus_stop() {
            assert_eq!(Interactive::focus(&popup(3, 8, 0)), FocusSpec::None);
            assert_eq!(Interactive::focus(&popup(30, 8, 0)), FocusSpec::Stop);
            let mut linked = popup(3, 8, 0);
            linked.links.push(RichTextLink {
                line: 0,
                start_byte: 0,
                end_byte: 4,
                url: "https://example.com".into(),
            });
            assert_eq!(Interactive::focus(&linked), FocusSpec::Stop);
        }

        #[test]
        fn a_click_on_a_link_reports_its_url_and_takes_focus() {
            let mut p = popup(3, 8, 0);
            p.links.push(RichTextLink {
                line: 1,
                start_byte: 0,
                end_byte: 4,
                url: "https://example.com".into(),
            });
            let layout = layout_for(&p, Rect::new(0.0, 0.0, 20.0, 5.0));
            let hit = layout.layout.link_hit_regions[0].0;
            let (out, host) = handle(&p, &down_at(hit.x + 0.5, hit.y + 0.5), &layout);
            assert_eq!(
                out,
                Handled::Event(RichTextPopupEvent::LinkClicked {
                    url: "https://example.com".into()
                })
            );
            assert_eq!(host.focus_requests, vec![None]);
        }

        #[test]
        fn a_click_on_the_body_is_consumed_and_a_click_outside_is_not() {
            let p = popup(3, 8, 0);
            let layout = layout_for(&p, Rect::new(0.0, 0.0, 20.0, 5.0));
            let (inside, host) = handle(&p, &down_at(2.0, 1.5), &layout);
            assert_eq!(inside, Handled::Consumed);
            assert_eq!(host.focus_requests, vec![None]);
            let (outside, _) = handle(&p, &down_at(2.0, 40.0), &layout);
            assert_eq!(outside, Handled::Ignored);
        }

        #[test]
        fn char_at_local_resolves_a_line_and_column() {
            let p = popup(3, 8, 0);
            let layout = layout_for(&p, Rect::new(4.0, 4.0, 20.0, 5.0));
            assert_eq!(layout.char_at_local(1.0, 1.5), Some((0, 0)));
            assert_eq!(layout.char_at_local(4.0, 2.5), Some((1, 3)));
            assert_eq!(layout.char_at_local(1.0, 40.0), None);
        }

        #[test]
        fn a_wheel_gesture_scrolls_the_body() {
            let p = popup(30, 4, 0);
            let layout = layout_for(&p, Rect::new(0.0, 0.0, 20.0, 6.0));
            let wheel = |dy: f32| RoutedEvent::Wheel {
                dx: 0.0,
                dy,
                local: Point::new(1.0, 1.0),
                mods: Modifiers::default(),
            };
            // `UiEvent::Scroll`'s convention is positive-y-is-up, so a
            // downward notch (-1.0) scrolls the body forward.
            let (down, _) = handle(&p, &wheel(-1.0), &layout);
            assert_eq!(
                down,
                Handled::Event(RichTextPopupEvent::Scrolled { new_offset: 1 })
            );
            // Already at the top: the popup owns the gesture but reports
            // nothing to apply.
            let (up, _) = handle(&p, &wheel(1.0), &layout);
            assert_eq!(up, Handled::Consumed);
        }

        #[test]
        fn page_keys_scroll_by_the_rows_actually_on_screen() {
            let p = popup(30, 4, 0);
            let layout = layout_for(&p, Rect::new(0.0, 0.0, 20.0, 6.0));
            assert_eq!(layout.visible_rows, 4);
            let (down, _) = handle(&p, &key(NamedKey::PageDown), &layout);
            assert_eq!(
                down,
                Handled::Event(RichTextPopupEvent::Scrolled { new_offset: 4 })
            );
        }

        /// Scrolling never runs off the end into blank rows: a Page key
        /// at the bottom pins the offset to the last full screen, and a
        /// stale `scroll_top` past it is reported back clamped.
        #[test]
        fn scrolling_stops_at_the_last_full_screen() {
            let layout = layout_for(&popup(30, 4, 0), Rect::new(0.0, 0.0, 20.0, 6.0));
            assert_eq!(layout.max_scroll(30), 26);

            let at_bottom = popup(30, 4, 26);
            let (pinned, _) = handle(&at_bottom, &key(NamedKey::PageDown), &layout);
            assert_eq!(pinned, Handled::Consumed);

            let stale = popup(30, 4, 99);
            let (corrected, _) = handle(&stale, &key(NamedKey::PageDown), &layout);
            assert_eq!(
                corrected,
                Handled::Event(RichTextPopupEvent::Scrolled { new_offset: 26 })
            );
        }

        #[test]
        fn a_body_that_fits_ignores_scroll_gestures() {
            let p = popup(3, 8, 0);
            let layout = layout_for(&p, Rect::new(0.0, 0.0, 20.0, 10.0));
            let (out, _) = handle(&p, &key(NamedKey::PageDown), &layout);
            assert_eq!(out, Handled::Ignored);
        }

        /// Escape must keep travelling: dismissing the popup is the
        /// host's policy, not the widget's.
        #[test]
        fn escape_is_not_claimed() {
            let p = popup(30, 4, 0);
            let layout = layout_for(&p, Rect::new(0.0, 0.0, 20.0, 6.0));
            let (out, _) = handle(&p, &key(NamedKey::Escape), &layout);
            assert_eq!(out, Handled::Ignored);
        }

        #[test]
        fn sub_regions_name_every_visible_link_in_local_space() {
            let mut p = popup(3, 8, 0);
            p.links.push(RichTextLink {
                line: 0,
                start_byte: 0,
                end_byte: 4,
                url: "https://example.com".into(),
            });
            let layout = layout_for(&p, Rect::new(5.0, 5.0, 20.0, 5.0));
            let regions = Interactive::sub_regions(&p, &layout);
            assert_eq!(regions.len(), 1);
            assert_eq!(regions[0].1, SubTarget(0));
            assert_eq!(regions[0].0, Rect::new(1.0, 1.0, 4.0, 1.0));
        }

        #[test]
        fn actions_map_onto_the_crates_own_ui_events() {
            let id = WidgetId::new("rtp:test");
            assert_eq!(
                RichTextPopupEvent::LinkClicked {
                    url: "https://example.com".into()
                }
                .into_ui_event(id),
                crate::UiEvent::RichTextLinkClicked(id, "https://example.com".into())
            );
            assert_eq!(
                RichTextPopupEvent::Scrolled { new_offset: 7 }.into_ui_event(id),
                crate::UiEvent::ScrollOffsetChanged {
                    widget: id,
                    new_offset: 7
                }
            );
        }
    }
}

/// Cell-painting entry points used by this module's own `#[cfg(test)]`
/// rasteriser tests: they build the `Buffer` -> [`crate::CellSurface`]
/// adapter (private to `crate::tui`) the same way
/// [`crate::primitive::Primitive::paint_cells`] does, so a test can
/// paint into a bare ratatui buffer without a backend.
#[cfg(all(test, feature = "tui"))]
mod cell_test_paint {
    use super::*;
    use crate::theme::Theme;
    use ratatui::buffer::Buffer;

    fn surface<'a>(
        buf: &'a mut Buffer,
        sink: &'a mut Option<(u16, u16, crate::cell_surface::CursorShape)>,
    ) -> impl crate::CellSurface + 'a {
        crate::tui::cell_surface::TuiCellSurface::new(
            buf,
            crate::backend::ColorDepth::TrueColor,
            sink,
        )
    }

    pub fn draw_rich_text_popup(
        buf: &mut Buffer,
        popup: &RichTextPopup,
        layout: &RichTextPopupLayout,
        theme: &Theme,
    ) {
        let mut sink = None;
        cell_paint::paint(popup, layout, &mut surface(buf, &mut sink), theme);
    }
}
