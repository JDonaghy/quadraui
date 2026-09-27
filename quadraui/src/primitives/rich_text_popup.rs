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
use crate::types::{Color, StyledText, WidgetId};
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
            // Thumb geometry via the one canonical formula (quadraui#820)
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

// ── NativeSurface Phase 4 slice 4/8 (#1077) ─────────────────────────────────
//
// `paint` below is shared by the **macOS and Windows** rasterisers only
// — `gtk::rich_text_popup::draw_rich_text_popup` is **not** migrated,
// and stays a full, bespoke Cairo + Pango implementation. This is a
// deliberate exception to this issue's "no per-backend paint loop left"
// acceptance bar, not an oversight — see that module's own doc for the
// full reasoning, summarised here:
//
// GTK renders each visible line with a **single** Pango call carrying a
// per-character `AttrList` (fg/bold/italic ranges, selection-inverted
// fg, focused-link underline, font-scale), specifically *because* an
// earlier per-span "measure each span, advance x by its width" approach
// (the same shape `paint` below uses) was found to drift from Pango's
// real shaped-line glyph positions for proportional fonts (issue #214:
// "the per-span manual-advance bug where proportional Pango widths
// drift from monospace char_width * char_count math"). Adjacent runs
// shaped separately and summed can differ from the same text shaped as
// one line (kerning/ligatures aren't strictly compositional) — GTK's
// fix was to stop manually advancing at all and let Pango shape+position
// the whole line, using `index_to_pos` afterward only to *locate* spans
// it already painted.
//
// `NativeSurface` has no verb for "shape one line with N attribute
// ranges, then ask where each range landed" — only single-color/single-
// style runs (`surface_draw_text_run(_styled)`). Migrating GTK onto
// `paint` below would mean going back to the per-span manual-advance
// shape #214 fixed, i.e. deliberately reintroducing a previously-fixed
// bug, or extending `NativeSurface` with a new attributed-line verb —
// a real, separate design decision outside this issue's scope (#1073's
// verb set), not a mechanical "move the code" migration. Per this
// issue's own "do not tranche silently" instruction: this is that call,
// made explicitly rather than by quietly skipping the primitive.
//
// macOS and Windows never had that problem — both already rendered
// per-span with manual x-advance (the same shape `paint` below takes),
// so consolidating *their* two copies carries no such risk. Doing so
// closes real gaps between them, resolved by adopting the richer side
// (2-of-3 majority pattern already established by
// `crate::primitives::palette::native_surface_paint`,
// `crate::primitives::tooltip::native_surface_paint`,
// `crate::primitives::dialog::native_surface_paint`):
//
// - **Selection background + inverted fg.** Windows already painted a
//   solid selection-bg rect and swapped a selected span's colour to the
//   popup bg; macOS's own module doc listed this as a "Scope omission."
//   `paint` carries it for both now.
// - **Bold span styling.** Windows already read `span.bold`; macOS's
//   doc listed this as a scope omission too. `paint` reads it uniformly
//   via `NativeSurface::surface_draw_text_run_styled` — macOS's own
//   adapter (`CgSurface`) takes that verb's *default*, which drops
//   style entirely (see that default's own doc), so this is inert on
//   macOS: no visual regression, but no new bold rendering there either,
//   matching every other primitive this default has ever applied to.
// - **Focused-link underline.** Windows already underlined the whole
//   span overlapping the focused link's byte range (coarser than GTK's
//   exact-substring underline, which needs `index_to_pos` — out of
//   reach here for the same reason as above); macOS's doc listed no
//   underline at all as a scope omission. `paint` carries Windows's
//   whole-span approximation for both now.
// - **Content-area clipping.** GTK and macOS both clip line/selection
//   painting to `layout.content_bounds` so an overlong line can't bleed
//   past the popup's own border; Windows painted unclipped. `paint`
//   clips on every backend now.
// - **Scrollbar width + track opacity.** GTK and macOS both paint a
//   [`SB_WIDTH`]-wide, [`SB_INSET`]-inset bar (wider than the
//   primitive's own `layout.scrollbar` geometry, which is a bare 1-unit
//   track — see that field's own doc — "wide enough to paint and click
//   easily" is a rasteriser-level choice layered on top) at
//   `theme.muted_fg` translucent at `0.3` alpha for the track; Windows
//   painted `layout.scrollbar.track`/`.thumb` verbatim (the primitive's
//   thin 1-unit geometry) at fully opaque `theme.muted_fg`. `paint`
//   adopts the wider, translucent-track treatment for every backend.
//   (This does not change hit-testing: `RichTextPopupLayout::hit_test`
//   already only recognises the primitive's own 1-unit
//   `layout.scrollbar` geometry on every backend, GTK included — a
//   pre-existing paint/click geometry mismatch this migration doesn't
//   introduce or fix, out of scope here.)
//
// Not carried over on macOS/Windows, matching GTK's own capability
// gap for the same reason: per-line font scale (`RichTextPopup::line_scales`,
// markdown heading rows) needs a per-row font swap
// (`CTFontCreateCopyWithSymbolicTraits` / a scaled `IDWriteTextFormat`)
// that neither backend's adapter can do mid-line without becoming a
// live-backend-only capability like `Dialog`'s `FontRole` — a bigger
// change than this slice attempts. `RichTextPopup::layout` still
// reserves the taller row height on any backend advertising
// `scale_rows`; only the *glyph* stays regular-sized.
#[cfg(any(feature = "win", all(feature = "macos", target_os = "macos")))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{RichTextPopup, RichTextPopupLayout, TextSelection};
    use crate::native_surface::NativeSurface;
    use crate::theme::Theme;
    use crate::{Point, Rect};

    /// Visible width of the rich-text-popup scrollbar, shared by
    /// [`paint`] and each backend's own hit-testing constant of the
    /// same value (`gtk`/`macos::rich_text_popup::RICH_TEXT_POPUP_SB_WIDTH`)
    /// — wider than [`RichTextPopupLayout::scrollbar`]'s bare 1-unit
    /// track so the bar is paint+click-friendly. See this module's doc
    /// for the drift this closes on Windows.
    pub(crate) const SB_WIDTH: f32 = 8.0;
    /// Inset between the scrollbar's right edge and the popup's own
    /// right border. See [`SB_WIDTH`]'s doc.
    pub(crate) const SB_INSET: f32 = 1.0;

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
        let char_to_byte = |col: usize| -> usize {
            line_text
                .char_indices()
                .nth(col)
                .map(|(b, _)| b)
                .unwrap_or(line_text.len())
        };
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
        (char_to_byte(start_col), char_to_byte(end_col))
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
        surface: &mut dyn NativeSurface,
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

    // ── RichTextPopup primitive tests (#214) ─────────────────────────────

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
}
