//! `Tooltip` primitive: a short text popup anchored to an element.
//! Used for hover-hint text (activity bar items, status segments,
//! truncated tab labels, LSP hover results that are small enough to
//! inline rather than a full docblock).
//!
//! A `Tooltip` is paired with an **anchor** rectangle — the element it
//! describes. The layout method picks a position (`placement`) near the
//! anchor that keeps the tooltip inside the viewport.
//!
//! # Backend contract
//!
//! **Declarative + placement.** Apps decide when a tooltip shows
//! (hover delay, keyboard focus, etc.) and pass the current anchor +
//! content. The primitive's `layout()` chooses x/y based on preferred
//! placement, adjusting if it would overflow the viewport. Backends
//! render a box with the content at the resolved position.
//!
//! ## Border/title vocabulary is a *sidecar*, not a field on either struct
//!
//! [`TooltipBorder`] and an optional title are per-tooltip chrome a
//! consumer can now ask for (`Sides` bars-only, `Full` closed box, `None`
//! no chrome at all; an optional title centred into `Full`'s top border
//! row). They live together in [`TooltipChrome`], a separate type
//! passed *alongside* the tooltip and its layout to
//! [`crate::Backend::draw_tooltip_with_chrome`] (and the per-backend
//! `draw_tooltip_with_chrome` rasterisers).
//!
//! They are deliberately **not** fields on [`Tooltip`] and **not** fields
//! on [`TooltipLayout`]. Both of those are plain, non-`#[non_exhaustive]`,
//! all-`pub`-field structs, and both are constructed with *exhaustive*
//! literals — `Tooltip { .. }` by several call sites, and
//! `TooltipLayout { bounds, resolved_placement }` by hand where a popup
//! centres itself over an area rather than anchoring to an element, so it
//! cannot use [`Tooltip::layout`] and builds the layout value directly —
//! its public fields exist for exactly that. Adding a required field to
//! *either* struct breaks every one of those literals the instant it
//! lands, and Rust offers no shim that keeps an exhaustive literal
//! compiling across an added field:
//!
//! - `Default` + `..Default::default()` only helps literals that already
//!   spread, which these don't.
//! - `#[non_exhaustive]` applied retroactively swaps `E0063` for
//!   `E0639: cannot construct non-exhaustive struct outside its crate` —
//!   strictly worse for code that *needs* to construct one.
//!
//! Threading the vocabulary through a separate value sidesteps the break
//! entirely. Nothing about `Tooltip`'s or `TooltipLayout`'s field set
//! changes, so every existing exhaustive literal of either — in tree and
//! downstream — keeps compiling untouched, and the new capability is
//! reached by calling a new method instead of by filling in a new field.
//!
//! [`crate::Backend::draw_tooltip`] keeps its exact signature and
//! behaviour (it renders `TooltipChrome::default()`, i.e. the
//! [`TooltipBorder::Full`] box every backend already drew), so existing
//! `Backend` implementors and callers are untouched too;
//! `draw_tooltip_with_chrome` is an added trait method with a default body
//! that delegates to it.
//!
//! ## `with_*` builder removal
//!
//! `Tooltip`'s `with_styled_lines` / `with_placement` / `with_bg` /
//! `with_fg` and `TooltipChrome`'s `with_border` / `with_title` builders
//! were pure convenience sugar around a field write, not something that
//! guarded an invariant — every one of those fields is already `pub`.
//! They were removed because they were not used. Set the field directly
//! instead — `TooltipChrome` is `#[non_exhaustive]` + `Default` already,
//! so it *is* the options-struct shape, and a future chrome/tooltip field
//! is additive either way. `Tooltip` itself stays a plain,
//! non-`#[non_exhaustive]` struct — that part of the "adding a field must
//! not break" goal already lives on `TooltipChrome`, which is why new
//! tooltip knobs have gone there rather than onto `Tooltip` since.
//!
//! Usage:
//! ```
//! # use quadraui::{Tooltip, TooltipBorder, TooltipChrome, TooltipMeasure, TooltipPlacement, WidgetId, Rect};
//! let mut tip = Tooltip::new(WidgetId::new("hover"), "Hover hint");
//! tip.placement = TooltipPlacement::Top;
//! let anchor = Rect::new(0.0, 0.0, 10.0, 1.0);
//! let viewport = Rect::new(0.0, 0.0, 80.0, 24.0);
//! let measure = TooltipMeasure::new(20.0, 3.0);
//! let layout = tip.layout(anchor, viewport, measure, 0.0);
//! let mut chrome = TooltipChrome::new(TooltipBorder::Sides);
//! chrome.title = Some("Hover hint".to_string());
//! // `backend.draw_tooltip_with_chrome(&tip, &layout, &chrome);`
//! assert_eq!(chrome.border, TooltipBorder::Sides);
//! ```

use crate::event::Rect;
use crate::layout::{Anchor, ResolvedSide, Side};
use crate::types::{Color, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a tooltip.
///
/// # Examples
///
/// ```
/// use quadraui::{Rect, ResolvedPlacement, Tooltip, TooltipMeasure, TooltipPlacement, WidgetId};
///
/// let mut tip = Tooltip::new(WidgetId::new("tooltip:hover"), "Hover hint");
/// tip.placement = TooltipPlacement::Top;
///
/// // Anchor sits well inside the viewport so the `Top` candidate has
/// // room above it and the preferred placement actually fits, rather
/// // than falling through to the "clamped to viewport" branch.
/// let anchor = Rect::new(20.0, 10.0, 10.0, 1.0);
/// let viewport = Rect::new(0.0, 0.0, 80.0, 24.0);
/// let layout = tip.layout(anchor, viewport, TooltipMeasure::new(20.0, 3.0), 0.0);
///
/// assert_eq!(layout.resolved_placement, ResolvedPlacement::Top);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tooltip {
    pub id: WidgetId,
    /// Tooltip text. May be multi-line (`\n`-separated); backends render
    /// each line in sequence. Plain text path — for per-character
    /// colour, set `styled_lines` instead.
    pub text: String,
    /// Multi-line styled override. When `Some`, backends render each
    /// `StyledText` as a separate row in sequence (with per-span fg/bg)
    /// instead of using the plain `text`. Single-line consumers (e.g.
    /// LSP signature help with one highlighted parameter) wrap their
    /// styled line in a 1-element vec; multi-line consumers (e.g.
    /// inline diff peek) supply one entry per row.
    #[serde(default)]
    pub styled_lines: Option<Vec<StyledText>>,
    /// Preferred placement relative to the anchor.
    #[serde(default)]
    pub placement: TooltipPlacement,
    /// Override background colour. `None` = theme default.
    #[serde(default)]
    pub bg: Option<Color>,
    /// Override foreground colour.
    #[serde(default)]
    pub fg: Option<Color>,
}

/// Border chrome vocabulary for [`Tooltip`].
///
/// Each backend can ask for a particular border style: side bars only,
/// a full 4-sided box, or no border at all. An optional title is shown
/// when the border is `Full` — how depends on the backend: TUI centres it
/// into the top border row (a terminal titled box); every pixel backend
/// (GTK/macOS/Win, via [`native_surface_paint::paint`]) instead renders it
/// as a bold first line *inside* the rounded card, left-aligned with the
/// body text below it.
///
/// `#[non_exhaustive]`: marking it means a future variant (e.g. a
/// `Rounded` corner style) is additive instead of a breaking change.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TooltipBorder {
    /// Vertical bars on the left/right edge only — no top or bottom rule,
    /// regardless of box height. `title` is not rendered in this mode.
    Sides,
    /// A closed box on all four sides. The default: all backends stroke
    /// a full rectangle here when there is room; on TUI, if the box is
    /// too short (`height < 3` or `width < 2`), the border falls back to
    /// `Sides` chrome so the tooltip still shows border.
    ///
    /// Carries `title`, when set — see this enum's own doc for where it
    /// lands on TUI vs. every pixel backend.
    #[default]
    Full,
    /// No border chrome at all — background fill (and text) only.
    /// `title` is not rendered in this mode.
    None,
}

/// Preferred placement of a `Tooltip` relative to its anchor.
///
/// The layout method falls back to the opposite side if the preferred
/// placement would overflow the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TooltipPlacement {
    /// Above the anchor, left-aligned.
    Top,
    /// Below the anchor, left-aligned.
    #[default]
    Bottom,
    /// Left of the anchor, vertically centered.
    Left,
    /// Right of the anchor, vertically centered.
    Right,
}

// ── D6 Layout API ───────────────────────────────────────────────────────────

/// Measurement for a `Tooltip` — the **total box size** the caller wants
/// reserved on screen, in the active backend's own units (character cells
/// on TUI, pixels on GTK).
///
/// # Contract
///
/// `height` is the whole tooltip box, border chrome included — not just
/// the content. `Tooltip::layout` passes it straight through to
/// [`TooltipLayout::bounds`], and that is what a backend's `draw_tooltip`
/// paints into. Concretely, on TUI:
///
/// - When `height >= 3` (and `width >= 2`), the TUI backend
///   strokes a full 4-sided box — one row each for the top and bottom
///   border — leaving only `height - 2` rows for content. GTK has always
///   stroked a full box the same way, but pays for it in sub-cell pixels
///   rather than whole rows, so it needs no equivalent padding.
/// - Below that (`height < 3`), TUI falls back to side-bars-only chrome
///   (`│` on the first/last column, no top/bottom rule) and all `height`
///   rows are usable for content.
///
/// **Callers that want *N* content lines visible on TUI once a bordered
/// box is drawn must pass `height >= N + 2`**, not `N`. Tooltips 3+ rows
/// tall draw a full 4-sided border, consuming 2 rows; passing a bare
/// content-line count will silently lose the last two lines.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TooltipMeasure {
    pub width: f32,
    pub height: f32,
}

impl TooltipMeasure {
    /// `height` is the total box height (border rows included on
    /// backends that reserve whole rows for chrome) — see the
    /// contract note on [`TooltipMeasure`] itself before passing a bare
    /// content-line count.
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// Resolved placement — what the layout actually chose (may differ
/// from `tooltip.placement` if the preferred direction overflowed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedPlacement {
    Top,
    Bottom,
    Left,
    Right,
}

/// Classification of a hit-test result on a tooltip. Tooltips are
/// non-interactive, so hits just report "on tooltip" vs "outside."
/// Apps that want to pin the tooltip on click use this as the signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TooltipHit {
    Body(WidgetId),
    Empty,
}

/// Fully-resolved tooltip layout.
///
/// Both fields are `pub` and this struct is not `#[non_exhaustive]`,
/// because consumers that position a popup themselves (rather than
/// anchoring it to an element via [`Tooltip::layout`]) construct one
/// directly with an exhaustive literal. Border and title vocabulary
/// therefore lives in the separate [`TooltipChrome`] sidecar rather than
/// as fields here — see the module doc.
#[derive(Debug, Clone, PartialEq)]
pub struct TooltipLayout {
    pub bounds: Rect,
    pub resolved_placement: ResolvedPlacement,
}

impl TooltipLayout {
    pub fn hit_test(&self, x: f32, y: f32, id: &WidgetId) -> TooltipHit {
        if x >= self.bounds.x
            && x < self.bounds.x + self.bounds.width
            && y >= self.bounds.y
            && y < self.bounds.y + self.bounds.height
        {
            TooltipHit::Body(*id)
        } else {
            TooltipHit::Empty
        }
    }
}

/// Per-tooltip chrome request: which border a backend should
/// stroke, and an optional title to embed in it.
///
/// Passed alongside a [`Tooltip`] + [`TooltipLayout`] to
/// [`crate::Backend::draw_tooltip_with_chrome`]. It is a separate value
/// rather than fields on either of those structs so that the change adds
/// no required field to a publicly-literal-constructible type — see the
/// module doc for the full reasoning.
///
/// [`TooltipChrome::default()`] is `Full` with no title: exactly what
/// every backend drew unconditionally before this type existed, which is
/// why [`crate::Backend::draw_tooltip`] can keep its old signature and
/// simply render the default.
///
/// `#[non_exhaustive]`: marking it means future chrome knobs — a corner
/// radius, a border colour override — are additive rather than breaking
/// changes. Construct with [`TooltipChrome::new`] /
/// [`TooltipChrome::default`], then set `border` / `title` directly —
/// both fields are `pub` and the type is already `#[non_exhaustive]` +
/// `Default`, so it *is* the options struct.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TooltipChrome {
    /// Which border to stroke. Defaults to [`TooltipBorder::Full`].
    #[serde(default)]
    pub border: TooltipBorder,
    /// Optional title, centred into the top border row when `border` is
    /// [`TooltipBorder::Full`]. Ignored by `Sides` and `None`, which have
    /// no top rule to embed it in.
    #[serde(default)]
    pub title: Option<String>,
}

impl TooltipChrome {
    /// Chrome with the given border and no title.
    pub fn new(border: TooltipBorder) -> Self {
        Self {
            border,
            title: None,
        }
    }
}

impl Tooltip {
    /// Construct a `Tooltip` with just the two fields every consumer must
    /// supply — `id` and `text` — and every other field at its
    /// behaviour-preserving default (`styled_lines: None`, `placement:
    /// Bottom`, `bg: None`, `fg: None`).
    pub fn new(id: WidgetId, text: impl Into<String>) -> Self {
        Self {
            id,
            text: text.into(),
            styled_lines: None,
            placement: TooltipPlacement::default(),
            bg: None,
            fg: None,
        }
    }

    /// Compute tooltip placement.
    ///
    /// # Arguments
    ///
    /// - `anchor` — bounds of the element being described.
    /// - `viewport` — bounds of the parent surface; tooltip is clamped
    ///   to stay inside these.
    /// - `measure` — total box width/height (border chrome included, not
    ///   just content) — see the contract note on [`TooltipMeasure`].
    ///   `measure.height` is copied verbatim into the returned
    ///   [`TooltipLayout::bounds`], which is exactly what a backend's
    ///   `draw_tooltip` uses to decide how many rows it has for both
    ///   border and content.
    /// - `margin` — gap between the anchor and the tooltip along the
    ///   placement axis.
    ///
    /// # Placement fallback
    ///
    /// The preferred placement is tried first. If it would push the
    /// tooltip past a viewport edge, the opposite side is tried. If
    /// both fail (unusual — anchor in the middle of a tiny viewport),
    /// the tooltip is pinned to the viewport edge on the preferred side.
    ///
    /// # Border/title
    ///
    /// The returned [`TooltipLayout`] carries geometry only. Border chrome
    /// and an optional title are requested separately, via a
    /// [`TooltipChrome`] value passed alongside this layout to
    /// [`crate::Backend::draw_tooltip_with_chrome`]; drawing through plain
    /// [`crate::Backend::draw_tooltip`] uses `TooltipChrome::default()`
    /// ([`TooltipBorder::Full`], no title) — the behaviour every backend
    /// used unconditionally before a choice was introduced. See the module
    /// doc for why that vocabulary is a sidecar rather than a field here.
    pub fn layout(
        &self,
        anchor: Rect,
        viewport: Rect,
        measure: TooltipMeasure,
        margin: f32,
    ) -> TooltipLayout {
        let vw = measure.width;
        let vh = measure.height;

        let side = match self.placement {
            TooltipPlacement::Top => Side::Top,
            TooltipPlacement::Bottom => Side::Bottom,
            TooltipPlacement::Left => Side::Left,
            TooltipPlacement::Right => Side::Right,
        };

        let (origin, resolved_side) = Anchor::new(anchor, side)
            .with_margin(margin)
            .resolve((vw, vh), viewport);

        let resolved_placement = match resolved_side {
            ResolvedSide::Top => ResolvedPlacement::Top,
            ResolvedSide::Bottom => ResolvedPlacement::Bottom,
            ResolvedSide::Left => ResolvedPlacement::Left,
            ResolvedSide::Right => ResolvedPlacement::Right,
            ResolvedSide::AtPoint => {
                unreachable!("TooltipPlacement never maps to Side::AtPoint")
            }
        };

        TooltipLayout {
            bounds: Rect::new(origin.x, origin.y, vw, vh),
            resolved_placement,
        }
    }
}

// ── Widget contract: `Primitive` + `Interactive` ───────────────────────────
//
// `Tooltip` reaches the frame type-erased as `Surface::Widget`: its
// `Primitive::layout` takes the rect the host already resolved through
// [`Tooltip::layout`] (anchor, viewport, measure, margin — none of which
// live on the descriptor) as the box's final bounds, `paint` runs the
// shared pixel painter, and `paint_cells` runs the cell painter in
// [`cell_paint`]. A tooltip reacts to no input of its own — hover delay,
// pointer-leave and explicit dismissal all live in
// [`crate::compose::HoverTooltipTracker`], which decides whether the host
// builds a `Tooltip` at all — so it is `non_interactive!`.
//
// Both painters render [`TooltipChrome::default()`] on this path, since the
// chrome request is not a field of the descriptor.

/// Vertical inset between a [`TooltipBorder::Full`] card's stroked edge and
/// its content on a pixel surface — VS Code's own hover-card inset.
/// [`Tooltip`]'s `Primitive::measure` reserves it above and below the
/// content; the pixel painter shrinks it to fit a tightly-sized card.
const CARD_VPAD: f32 = 4.0;

impl TooltipPlacement {
    /// The [`ResolvedPlacement`] for this placement without a viewport
    /// flip — what a layout built from an already-placed rect reports.
    fn unflipped(self) -> ResolvedPlacement {
        match self {
            TooltipPlacement::Top => ResolvedPlacement::Top,
            TooltipPlacement::Bottom => ResolvedPlacement::Bottom,
            TooltipPlacement::Left => ResolvedPlacement::Left,
            TooltipPlacement::Right => ResolvedPlacement::Right,
        }
    }
}

impl Tooltip {
    /// How many content rows this tooltip paints: one per `styled_lines`
    /// entry, else one per `\n`-separated line of `text`.
    fn content_rows(&self) -> usize {
        match &self.styled_lines {
            Some(lines) => lines.len(),
            None => self.text.lines().count(),
        }
    }

    /// The widest content row, in terminal display columns.
    fn content_cols(&self) -> usize {
        use crate::text_util::display_width;
        match &self.styled_lines {
            Some(lines) => lines
                .iter()
                .map(|line| line.spans.iter().map(|s| display_width(&s.text)).sum())
                .max()
                .unwrap_or(0),
            None => self.text.lines().map(display_width).max().unwrap_or(0),
        }
    }
}

impl crate::primitive::Primitive for Tooltip {
    type Layout = TooltipLayout;
    type Event = TooltipHit;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Tooltip(self.clone())
    }

    /// The whole box — default [`TooltipBorder::Full`] chrome included —
    /// that fits every content row, following the [`TooltipMeasure`]
    /// contract. On a cell grid that is two leading columns (border +
    /// pad) and one trailing border column around the widest row, plus a
    /// top and bottom border row. On a pixel surface it is one
    /// `char_width` of padding either side of the widest row (estimated
    /// at `char_width` per column — no live text measurer reaches
    /// `measure`) and a [`CARD_VPAD`] inset above and below the rows.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        let rows = self.content_rows() as f32;
        let cols = self.content_cols() as f32;
        match ctx.units() {
            crate::primitive::Units::Cells => {
                crate::NaturalSize::new(Some(cols + 3.0), Some(rows + 2.0))
            }
            _ => crate::NaturalSize::new(
                Some((cols + 2.0) * ctx.char_width),
                Some(rows * ctx.line_height + 2.0 * CARD_VPAD),
            ),
        }
    }

    /// `rect` is the tooltip's final box — the `bounds` a host got from
    /// [`Tooltip::layout`] — reported with the preferred placement as
    /// the resolved one.
    fn layout(&self, _ctx: &crate::primitive::LayoutCtx, rect: Rect) -> TooltipLayout {
        TooltipLayout {
            bounds: rect,
            resolved_placement: self.placement.unflipped(),
        }
    }

    /// The shared pixel painter, with the surface's own line height and
    /// character advance as the row pitch and horizontal padding — the
    /// same metrics every pixel backend's `draw_tooltip` passes it.
    fn paint(
        &self,
        layout: &TooltipLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        theme: &crate::theme::Theme,
        style: &crate::style::Style,
    ) {
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            let line_height = surface.surface_line_height();
            let padding_x = surface.surface_char_width();
            native_surface_paint::paint(
                self,
                layout,
                &TooltipChrome::default(),
                line_height,
                padding_x,
                surface,
                theme,
                style,
            );
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme, style);
        }
    }

    /// [`cell_paint::paint`] over `buf`, reached through
    /// [`crate::tui::draw_tooltip`] because the `Buffer` → `CellSurface`
    /// adapter is private to `crate::tui`.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &TooltipLayout,
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
        cell_paint::paint(self, layout, &TooltipChrome::default(), &mut surface, theme);
    }
}

crate::non_interactive!(Tooltip);

/// The cell-grid painter for [`Tooltip`], written against
/// [`crate::CellSurface`].
///
/// Border chrome follows the [`TooltipChrome`] request:
///
/// - [`TooltipBorder::Full`] (the default) renders a closed box —
///   `┌─┐`/`└─┘` top and bottom rows plus `│` sides — whenever the box
///   leaves room for both border rows and at least one content row
///   (`height >= 3`, `width >= 2`). Below that it falls back to the
///   `Sides` rendering rather than an empty box. An optional
///   `chrome.title` is centred into the top row when the box closes.
/// - [`TooltipBorder::Sides`] renders `│` on the first/last column only,
///   regardless of height. No title (no top rule to embed it in).
/// - [`TooltipBorder::None`] renders no border chrome at all.
///
/// When `tooltip.styled_lines` is `Some`, each entry renders as one row of
/// styled spans. Otherwise `tooltip.text` is split on `\n` and each line
/// renders plain. Lines wider than the box or beyond the available
/// content rows are truncated.
///
/// `layout.bounds.height` is the *total* box height, not a content-row
/// count — see the contract note on [`TooltipMeasure`]. A caller that
/// wants `N` content lines visible once a bordered box is drawn
/// (`height >= 3`) must measure `N + 2`.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{Tooltip, TooltipBorder, TooltipChrome, TooltipLayout};
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::event::Rect;
    use crate::theme::Theme;
    use crate::types::Color;

    /// The whole-cell bounds [`paint`] actually paints into —
    /// `layout.bounds` rounded to integer terminal columns/rows. A
    /// backend registers *this* as the tooltip's zone, so a
    /// structural-parity observer sees exactly the painted cells.
    pub(crate) fn painted_bounds(layout: &TooltipLayout) -> Rect {
        Rect::new(
            layout.bounds.x.round(),
            layout.bounds.y.round(),
            layout.bounds.width.round(),
            layout.bounds.height.round(),
        )
    }

    fn style(fg: Color, bg: Color) -> CellStyle {
        CellStyle {
            fg: Some(fg),
            bg: Some(bg),
            ..CellStyle::default()
        }
    }

    /// Paint `tooltip` into `layout`'s painted bounds on `surface`, with
    /// the border and optional title `chrome` requests. Per-tooltip
    /// `fg`/`bg` overrides win over the theme's hover colours; the border
    /// always uses [`Theme::hover_border`].
    pub(crate) fn paint(
        tooltip: &Tooltip,
        layout: &TooltipLayout,
        chrome: &TooltipChrome,
        surface: &mut dyn CellSurface,
        theme: &Theme,
    ) {
        let painted = painted_bounds(layout);
        let x = painted.x as u16;
        let y = painted.y as u16;
        let w = painted.width as u16;
        let h = painted.height as u16;
        if w == 0 || h == 0 {
            return;
        }

        let fg = tooltip.fg.unwrap_or(theme.hover_fg);
        let bg = tooltip.bg.unwrap_or(theme.hover_bg);
        let text_style = style(fg, bg);
        let border_style = style(theme.hover_border, bg);

        // `Sides` never draws top/bottom, regardless of height. `Full`
        // draws a closed box whenever there's room for both border rows
        // and at least one content row; below that it falls back to the
        // side bars `Sides` uses rather than rendering an empty box.
        // `w >= 2` keeps the left and right corner columns distinct.
        let has_horizontal_border =
            matches!(chrome.border, TooltipBorder::Full) && h >= 3 && w >= 2;
        let draw_side_bars = !matches!(chrome.border, TooltipBorder::None);
        let content_row0: u16 = if has_horizontal_border { 1 } else { 0 };
        let content_rows: u16 = if has_horizontal_border { h - 2 } else { h };
        // Content starts one cell past the side border when one is drawn
        // (border column + 1 pad), or one cell of padding when it isn't,
        // and stops one column short of the right edge.
        let text_col_offset: u16 = if draw_side_bars { 2 } else { 1 };
        let text_cols = w.saturating_sub(text_col_offset + 1);

        if has_horizontal_border {
            paint_border_row(
                surface,
                (x, y, w),
                true,
                chrome.title.as_deref(),
                border_style,
                text_style,
            );
            paint_border_row(
                surface,
                (x, y + h - 1, w),
                false,
                None,
                border_style,
                text_style,
            );
        }

        let paint_row_background = |surface: &mut dyn CellSurface, row: u16| {
            surface.fill(Rect::new(x as f32, row as f32, w as f32, 1.0), text_style);
            if draw_side_bars {
                surface.put_char(x, row, '│', border_style);
                surface.put_char(x + w - 1, row, '│', border_style);
            }
        };

        if let Some(ref styled_lines) = tooltip.styled_lines {
            for (i, styled) in styled_lines.iter().enumerate().take(content_rows as usize) {
                let row = y + content_row0 + i as u16;
                paint_row_background(surface, row);
                let mut col: u16 = 0;
                for span in &styled.spans {
                    let span_style = style(span.fg.unwrap_or(fg), span.bg.unwrap_or(bg));
                    col += surface.put_str_clipped(
                        x + text_col_offset + col,
                        row,
                        &span.text,
                        text_cols.saturating_sub(col),
                        span_style,
                    );
                }
            }
            return;
        }

        for (i, text_line) in tooltip.text.lines().enumerate().take(content_rows as usize) {
            let row = y + content_row0 + i as u16;
            paint_row_background(surface, row);
            surface.put_str_clipped(x + text_col_offset, row, text_line, text_cols, text_style);
        }
    }

    /// One horizontal border row of a closed box at `(x, row)`, `w`
    /// columns wide, with `title` — framed by a single space either side
    /// — centred among the interior columns. A title that doesn't fit the
    /// interior is dropped rather than clipped: a truncated title reads as
    /// a rendering bug, plain dashes don't. An odd remainder puts the
    /// title one column left of dead-centre, matching how the rest of
    /// this crate centres odd remainders.
    fn paint_border_row(
        surface: &mut dyn CellSurface,
        (x, row, w): (u16, u16, u16),
        top: bool,
        title: Option<&str>,
        border_style: CellStyle,
        title_style: CellStyle,
    ) {
        let (left, right) = if top { ('┌', '┐') } else { ('└', '┘') };
        for col in 0..w {
            let ch = if col == 0 {
                left
            } else if col == w - 1 {
                right
            } else {
                '─'
            };
            surface.put_char(x + col, row, ch, border_style);
        }

        let interior = w.saturating_sub(2);
        let Some(framed) = title.map(|t| format!(" {t} ")) else {
            return;
        };
        let title_len = framed.chars().count() as u16;
        if title_len > interior {
            return;
        }
        let title_start = 1 + (interior - title_len) / 2;
        surface.put_str_clipped(x + title_start, row, &framed, title_len, title_style);
    }
}

// ── PaintSurface Phase 4 slice 4/8 ─────────────────────────────────────────
//
// `paint` below is the one shared paint implementation, written against
// [`crate::paint_surface::PaintSurface`] instead of any one backend's
// API — see `crate::primitives::context_menu::native_surface_paint` for
// the same pattern applied one primitive earlier in this slice.
//
// Pre-migration, `gtk::tooltip`, `macos::tooltip` and `win::tooltip`
// were already near-identical (bg fill, border-chrome match on `Full` /
// `Sides` / `None`, optional title punched through the top rule,
// plain-`text`-vs-`styled_lines` content) — the only real drift found
// while unifying:
//
// - **Styled-line span weight/style.** [`crate::types::StyledSpan`]
//   carries `bold`/`italic`/`underline` per run. Windows's rasteriser
//   already measured and drew each span through
//   `DWrite::{measure_text_styled, draw_text_styled}`, honouring
//   `span.bold` (DirectWrite has no italic/underline draw path of its
//   own, so those two were not supported there); GTK and macOS both
//   ignored all three — GTK explicitly cleared Pango attributes before
//   drawing each span, and macOS's `draw_text`/`measure_text` never took
//   a style parameter at all. `paint` calls
//   [`crate::paint_surface::PaintSurface::surface_draw_text_run_styled`]
//   /`surface_measure_text_styled` for every backend now: Windows keeps
//   its bold rendering exactly as before (italic/underline not supported
//   per DirectWrite capabilities); GTK gains real bold/italic/underline
//   rendering it never had; macOS's adapter ignores style and forwards to
//   the plain measure/draw so its rendering is unchanged. Plain
//   `tooltip.text` (no `styled_lines`) is unaffected on every backend —
//   it never carried per-run style.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{Tooltip, TooltipBorder, TooltipChrome, TooltipLayout, CARD_VPAD};
    use crate::paint_surface::PaintSurface;
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::types::Color;
    use crate::{Point, Rect};

    /// Paint a [`Tooltip`] at its resolved `tooltip_layout` onto
    /// `surface`, with the border and optional title requested by
    /// `chrome`.
    ///
    /// `line_height` / `padding_x` are in `surface`-native units
    /// (character cells for TUI-shaped callers, pixels for every pixel
    /// backend — though TUI itself never calls this, see the module
    /// doc's *Why TUI stays out* on [`PaintSurface`]). `padding_x` is
    /// halved when `chrome.border` is [`TooltipBorder::None`], since
    /// there is no border column to clear first. `style` supplies the
    /// corner radius/border width/shadow elevation the box
    /// paints with when `chrome.border` is [`TooltipBorder::Full`] —
    /// callers pass `&self.style()`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        tooltip: &Tooltip,
        tooltip_layout: &TooltipLayout,
        chrome: &TooltipChrome,
        line_height: f32,
        padding_x: f32,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
    ) {
        let bounds = tooltip_layout.bounds;
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }

        let bg = tooltip.bg.unwrap_or(theme.hover_bg);
        let fg = tooltip.fg.unwrap_or(theme.hover_fg);
        let border = theme.hover_border;

        // Only the fully-bordered box gets the VS-Code-style
        // elevated-card treatment — `Sides`/`None` chrome has no closed
        // box for a shadow/radius to outline.
        if matches!(chrome.border, TooltipBorder::Full) {
            surface.surface_draw_shadow(
                bounds,
                style.corner_radius,
                style.shadow_elevation,
                Color::rgba(0, 0, 0, 100),
            );
            surface.surface_fill_rounded_rect(bounds, style.corner_radius, bg);
        } else {
            surface.surface_fill_rect(bounds, bg);
        }

        // `Full` is the VS-Code-style card: content starts a few pixels
        // inside the stroked edge, not flush against it — VS Code's own
        // hover-card vertical inset (distinct from `style.padding`'s 8px
        // *horizontal* body inset, see `text_padding_x` below). The inset
        // shrinks to fit when the box is sized tightly around its content
        // (half the spare height, never negative), so a caller that sizes
        // the box to just its rows still gets every row painted. `Sides`
        // and `None` have no top rule to clear, so content starts flush
        // with the box's top edge — callers following the
        // `TooltipMeasure` contract size those two variants to exactly the
        // content rows they hold, with no border row to spare.
        let title = if matches!(chrome.border, TooltipBorder::Full) {
            chrome
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
        } else {
            None
        };
        let title_size = title.map(|t| surface.surface_measure_text_styled(t, true));
        let mut text_top = match chrome.border {
            TooltipBorder::Full => {
                let body_rows = match tooltip.styled_lines {
                    Some(ref lines) => lines.len(),
                    None => tooltip.text.lines().count(),
                };
                let title_block = title_size.map_or(0.0, |(_, h)| h + CARD_VPAD);
                let content_h = title_block + body_rows as f32 * line_height;
                let spare = (bounds.height - content_h).max(0.0);
                bounds.y + CARD_VPAD.min(spare / 2.0)
            }
            TooltipBorder::Sides | TooltipBorder::None => bounds.y,
        };

        match chrome.border {
            TooltipBorder::Full => {
                surface.surface_stroke_rounded_rect(
                    bounds,
                    style.corner_radius,
                    border,
                    style.border_width,
                );
            }
            TooltipBorder::Sides => {
                surface.surface_draw_line(
                    Point::new(bounds.x, bounds.y),
                    Point::new(bounds.x, bounds.y + bounds.height),
                    border,
                    1.0,
                );
                surface.surface_draw_line(
                    Point::new(bounds.x + bounds.width, bounds.y),
                    Point::new(bounds.x + bounds.width, bounds.y + bounds.height),
                    border,
                    1.0,
                );
            }
            TooltipBorder::None => {}
        }

        let text_padding_x = if matches!(chrome.border, TooltipBorder::None) {
            padding_x / 2.0
        } else {
            padding_x
        };
        let text_x = bounds.x + text_padding_x;
        let text_w = (bounds.x + bounds.width - text_x).max(0.0);

        // Title: a bold first line *inside* the card, left-aligned with
        // the body text below it — not centred into (and punched through)
        // the border stroke the way a terminal titled box draws it. Only
        // `Full` chrome has a closed card for a title to sit inside;
        // `Sides`/`None` ignore it, per `TooltipChrome::title`'s doc.
        if let (Some(title), Some((title_w, title_h))) = (title, title_size) {
            // The title always starts below the border stroke, even when
            // a tightly-sized card leaves no spare height for the
            // vertical inset — otherwise a font whose ascent sits at the
            // top of its line box paints glyph ink onto the border row.
            text_top = text_top.max(bounds.y + style.border_width.max(1.0));
            surface.surface_draw_text_run_styled(
                Rect::new(
                    text_x,
                    text_top,
                    text_w.max(title_w).max(1.0),
                    title_h.max(1.0),
                ),
                title,
                fg,
                true,
                false,
                false,
                1.0,
            );
            // Gap between the bold title row and the body text below
            // it — the same vertical inset as the card's own top edge.
            text_top += title_h + CARD_VPAD;
        }

        if let Some(ref styled_lines) = tooltip.styled_lines {
            for (i, styled) in styled_lines.iter().enumerate() {
                let row_y = text_top + i as f32 * line_height;
                if row_y + line_height > bounds.y + bounds.height {
                    break;
                }
                let mut x_off = text_x;
                for span in &styled.spans {
                    let span_fg = span.fg.unwrap_or(fg);
                    let (span_w, _) = surface.surface_measure_text_styled(&span.text, span.bold);
                    let rect = Rect::new(x_off, row_y, span_w.max(1.0), line_height);
                    surface.surface_draw_text_run_styled(
                        rect,
                        &span.text,
                        span_fg,
                        span.bold,
                        span.italic,
                        span.underline,
                        1.0,
                    );
                    x_off += span_w;
                }
            }
            return;
        }

        for (i, text_line) in tooltip.text.lines().enumerate() {
            let row_y = text_top + i as f32 * line_height;
            if row_y + line_height > bounds.y + bounds.height {
                break;
            }
            surface.surface_draw_text_run(
                Rect::new(text_x, row_y, text_w, line_height),
                text_line,
                fg,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Tooltip primitive tests (D6 shape) ────────────────────────────

    /// The border/title vocabulary is a separate [`TooltipChrome`]
    /// value passed alongside the tooltip and its layout — *not* a field
    /// on `Tooltip` or on `TooltipLayout`, both of which consumers
    /// construct with exhaustive literals (see `primitives::tooltip`'s
    /// module doc). `TooltipChrome::default()` matches what every backend
    /// drew unconditionally before a choice was introduced.
    #[test]
    fn tooltip_chrome_defaults_to_full_then_fields_override() {
        let chrome = TooltipChrome::default();
        assert_eq!(chrome.border, TooltipBorder::Full);
        assert_eq!(chrome.title, None);

        let mut chrome = TooltipChrome::new(TooltipBorder::None);
        chrome.title = Some("Hi".to_string());
        assert_eq!(chrome.border, TooltipBorder::None);
        assert_eq!(chrome.title.as_deref(), Some("Hi"));

        chrome.border = TooltipBorder::Sides;
        assert_eq!(chrome.border, TooltipBorder::Sides);
    }

    /// `TooltipLayout` must stay constructible from a bare exhaustive
    /// literal, because downstream consumers that position a popup
    /// themselves (rather than anchoring it via `Tooltip::layout`) build
    /// one by hand. If a future change adds a required field here, this
    /// test stops compiling — which would signal an `E0063` break for
    /// every such call site.
    #[test]
    fn tooltip_layout_stays_exhaustively_constructible() {
        let layout = TooltipLayout {
            bounds: Rect::new(1.0, 2.0, 30.0, 4.0),
            resolved_placement: ResolvedPlacement::Bottom,
        };
        assert_eq!(layout.bounds.width, 30.0);
    }

    #[test]
    fn tooltip_layout_prefers_bottom_when_room() {
        let t = Tooltip {
            id: WidgetId::new("tip"),
            text: "Hello".to_string(),
            placement: TooltipPlacement::Bottom,
            styled_lines: None,
            bg: None,
            fg: None,
        };
        let anchor = Rect::new(100.0, 50.0, 40.0, 20.0);
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = t.layout(anchor, viewport, TooltipMeasure::new(60.0, 16.0), 4.0);
        assert_eq!(layout.resolved_placement, ResolvedPlacement::Bottom);
        // Bottom placement: y = anchor.y + anchor.height + margin = 50 + 20 + 4 = 74
        assert_eq!(layout.bounds.y, 74.0);
        // Centered horizontally on anchor: x = 100 + (40 - 60)/2 = 90
        assert_eq!(layout.bounds.x, 90.0);
    }

    #[test]
    fn tooltip_layout_flips_to_opposite_when_overflow() {
        // Anchor near bottom of viewport — preferred Bottom would overflow.
        let t = Tooltip {
            id: WidgetId::new("tip"),
            text: "Hello".to_string(),
            placement: TooltipPlacement::Bottom,
            styled_lines: None,
            bg: None,
            fg: None,
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 100.0);
        let anchor = Rect::new(100.0, 80.0, 40.0, 16.0);
        let layout = t.layout(anchor, viewport, TooltipMeasure::new(60.0, 16.0), 4.0);
        // Bottom would put tooltip at y=80+16+4=100 which exceeds viewport_height 100.
        // Flip to Top.
        assert_eq!(layout.resolved_placement, ResolvedPlacement::Top);
        // Top: y = anchor.y - margin - vh = 80 - 4 - 16 = 60
        assert_eq!(layout.bounds.y, 60.0);
    }

    #[test]
    fn tooltip_layout_clamped_when_neither_fits() {
        // Tiny viewport, anchor in middle — both Top and Bottom overflow
        // (viewport is shorter than anchor + margin + tooltip).
        let t = Tooltip {
            id: WidgetId::new("tip"),
            text: "…".to_string(),
            placement: TooltipPlacement::Bottom,
            styled_lines: None,
            bg: None,
            fg: None,
        };
        let viewport = Rect::new(0.0, 0.0, 100.0, 30.0);
        let anchor = Rect::new(10.0, 10.0, 20.0, 10.0);
        let layout = t.layout(anchor, viewport, TooltipMeasure::new(40.0, 20.0), 4.0);
        // Preferred (Bottom) clamped: y would be 10+10+4=24; tooltip_h=20 →
        // bottom edge at 44, > viewport 30. Doesn't fit. Try Top: y = 10-4-20=-14 → doesn't fit.
        // Clamp preferred (Bottom): y clamped to viewport.height - vh = 30 - 20 = 10.
        assert_eq!(layout.resolved_placement, ResolvedPlacement::Bottom);
        assert!(layout.bounds.y <= 10.0);
    }

    #[test]
    fn tooltip_layout_does_not_panic_when_wider_than_viewport() {
        // When content width exceeds viewport width, the tooltip is
        // pinned to the viewport edge rather than panicking.
        let t = Tooltip {
            id: WidgetId::new("tip"),
            text: "Very wide".to_string(),
            placement: TooltipPlacement::Top,
            styled_lines: None,
            bg: None,
            fg: None,
        };
        let anchor = Rect::new(50.0, 50.0, 10.0, 10.0);
        let viewport = Rect::new(0.0, 0.0, 100.0, 100.0);
        // Content much wider than viewport (300 vs 100).
        let layout = t.layout(anchor, viewport, TooltipMeasure::new(300.0, 16.0), 4.0);
        // Pinned to viewport.x rather than panicking; bounds carries the
        // requested size so the consumer can see the overflow.
        assert_eq!(layout.bounds.x, 0.0);
        assert_eq!(layout.bounds.width, 300.0);
    }

    #[test]
    fn tooltip_layout_does_not_panic_when_taller_than_viewport() {
        // Symmetric case for the y-axis.
        let t = Tooltip {
            id: WidgetId::new("tip"),
            text: "Very tall".to_string(),
            placement: TooltipPlacement::Left,
            styled_lines: None,
            bg: None,
            fg: None,
        };
        let anchor = Rect::new(50.0, 50.0, 10.0, 10.0);
        let viewport = Rect::new(0.0, 0.0, 100.0, 100.0);
        let layout = t.layout(anchor, viewport, TooltipMeasure::new(50.0, 500.0), 4.0);
        assert_eq!(layout.bounds.y, 0.0);
        assert_eq!(layout.bounds.height, 500.0);
    }

    #[test]
    fn tooltip_layout_hit_test() {
        let t = Tooltip {
            id: WidgetId::new("tip"),
            text: "Hover".to_string(),
            placement: TooltipPlacement::Right,
            styled_lines: None,
            bg: None,
            fg: None,
        };
        let anchor = Rect::new(100.0, 100.0, 20.0, 20.0);
        let viewport = Rect::new(0.0, 0.0, 400.0, 400.0);
        let layout = t.layout(anchor, viewport, TooltipMeasure::new(80.0, 16.0), 4.0);
        let center_x = layout.bounds.x + 10.0;
        let center_y = layout.bounds.y + 5.0;
        match layout.hit_test(center_x, center_y, &t.id) {
            TooltipHit::Body(id) => assert_eq!(id.as_str(), "tip"),
            _ => panic!(),
        }
        assert_eq!(layout.hit_test(0.0, 0.0, &t.id), TooltipHit::Empty);
    }

    // ── Widget contract (`Primitive` + `Interactive`) ─────────────────

    fn cell_ctx() -> crate::primitive::LayoutCtx<'static> {
        crate::primitive::LayoutCtx::new(1.0, 1.0, false)
    }

    /// `Primitive::layout` keeps the host-placed rect as the box and
    /// reports the preferred placement as the resolved one.
    #[test]
    fn primitive_layout_takes_the_rect_as_the_box() {
        use crate::primitive::Primitive;
        let mut tip = Tooltip::new(WidgetId::new("tip"), "hi");
        tip.placement = TooltipPlacement::Left;
        let rect = Rect::new(3.0, 4.0, 12.0, 3.0);
        let layout = Primitive::layout(&tip, &cell_ctx(), rect);
        assert_eq!(layout.bounds, rect);
        assert_eq!(layout.resolved_placement, ResolvedPlacement::Left);
        assert_eq!(Primitive::id(&tip), Some(&tip.id));
    }

    /// On a cell grid the natural box is the widest row plus border and
    /// padding columns, and every row plus the two border rows.
    #[test]
    fn primitive_measure_in_cells_wraps_the_widest_row_in_the_full_box() {
        use crate::primitive::{Primitive, Size};
        let tip = Tooltip::new(WidgetId::new("tip"), "short\na longer line");
        let size = Primitive::measure(&tip, &cell_ctx(), Size::new(80.0, 24.0));
        assert_eq!(size.width, Some("a longer line".len() as f32 + 3.0));
        assert_eq!(size.height, Some(4.0));
    }

    /// Styled lines measure by the summed width of their spans.
    #[test]
    fn primitive_measure_counts_styled_spans() {
        use crate::primitive::{Primitive, Size};
        use crate::types::StyledSpan;
        let mut tip = Tooltip::new(WidgetId::new("tip"), "");
        tip.styled_lines = Some(vec![StyledText {
            spans: vec![StyledSpan::plain("fn "), StyledSpan::plain("foo()")],
        }]);
        let size = Primitive::measure(&tip, &cell_ctx(), Size::new(80.0, 24.0));
        assert_eq!(size.width, Some(8.0 + 3.0));
        assert_eq!(size.height, Some(3.0));
    }

    /// On a pixel surface the natural box pads the rows by one character
    /// advance either side and by the card inset above and below.
    #[test]
    fn primitive_measure_in_pixels_pads_by_char_width_and_card_inset() {
        use crate::primitive::{LayoutCtx, Primitive, Size};
        let tip = Tooltip::new(WidgetId::new("tip"), "abcd\nef");
        let ctx = LayoutCtx::new(16.0, 8.0, false);
        let size = Primitive::measure(&tip, &ctx, Size::new(800.0, 600.0));
        assert_eq!(size.width, Some((4.0 + 2.0) * 8.0));
        assert_eq!(size.height, Some(2.0 * 16.0 + 2.0 * CARD_VPAD));
    }

    /// A tooltip takes no focus and ignores every routed event.
    #[test]
    fn tooltip_is_non_interactive() {
        use crate::interactive::{FocusSpec, Interactive};
        let tip = Tooltip::new(WidgetId::new("tip"), "hi");
        assert_eq!(Interactive::focus(&tip), FocusSpec::None);
    }

    /// `Primitive::capture` round-trips through the tooltip's own
    /// `OwnedContent` variant.
    #[test]
    fn primitive_capture_is_the_tooltip_content() {
        use crate::primitive::Primitive;
        let tip = Tooltip::new(WidgetId::new("tip"), "hi");
        match Primitive::capture(&tip) {
            crate::frame::OwnedContent::Tooltip(captured) => assert_eq!(captured, tip),
            other => panic!("expected OwnedContent::Tooltip, got {other:?}"),
        }
    }

    // ── Pixel-path card paint (`native_surface_paint::paint`) ──────────

    #[cfg(any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    mod pixel_card {
        use super::*;
        use crate::paint_surface::PaintSurface;
        use crate::style::Style;
        use crate::theme::Theme;
        use crate::types::Color;
        use crate::{Point, Viewport};

        /// Records every drawn text run (plain or styled) and every
        /// filled rect, so a test can assert on what `paint` actually
        /// drew rather than an internal helper — the same shape as
        /// `context_menu`'s `RecordingSurface`.
        #[derive(Default)]
        struct RecordingSurface {
            /// `(rect, text, bold)` for every drawn run.
            runs: Vec<(Rect, String, bool)>,
            /// Every rounded-rect stroke (a `Full` card's border).
            rounded_strokes: Vec<Rect>,
            /// Every straight line (a `Sides` card's bars).
            lines: Vec<(Point, Point)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(0.0, 0.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 16.0)
            }
            fn surface_fill_rect(&mut self, _rect: Rect, _color: Color) {}
            fn surface_fill_rounded_rect(&mut self, _rect: Rect, _radius: f32, _color: Color) {}
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_stroke_rounded_rect(
                &mut self,
                rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
                self.rounded_strokes.push(rect);
            }
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, _color: Color) {
                self.runs.push((rect, text.to_string(), false));
            }
            #[allow(clippy::too_many_arguments)]
            fn surface_draw_text_run_styled(
                &mut self,
                rect: Rect,
                text: &str,
                _color: Color,
                bold: bool,
                _italic: bool,
                _underline: bool,
                _scale_x: f32,
            ) {
                self.runs.push((rect, text.to_string(), bold));
            }
            fn surface_draw_line(
                &mut self,
                from: Point,
                to: Point,
                _color: Color,
                _stroke_width: f32,
            ) {
                self.lines.push((from, to));
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(
                &mut self,
                _rect: Rect,
                _image: &crate::Image,
            ) -> crate::backend::ImagePaintResult {
                crate::backend::ImagePaintResult::Unsupported
            }
        }

        /// VS-Code-style default: a titled `Full` card renders its title
        /// as a bold first line *inside* the card bounds, not a label
        /// centred into (and punched through) the top border stroke the
        /// way a terminal titled box draws it.
        #[test]
        fn full_border_title_renders_bold_inside_card_bounds() {
            let tooltip = Tooltip::new(WidgetId::new("tip"), "Body text");
            let layout = TooltipLayout {
                bounds: Rect::new(10.0, 10.0, 120.0, 60.0),
                resolved_placement: ResolvedPlacement::Bottom,
            };
            let mut chrome = TooltipChrome::new(TooltipBorder::Full);
            chrome.title = Some("Title".to_string());

            let mut surface = RecordingSurface::default();
            native_surface_paint::paint(
                &tooltip,
                &layout,
                &chrome,
                16.0,
                8.0,
                &mut surface,
                &Theme::default(),
                &Style::default(),
            );

            let (title_rect, title_text, bold) = surface
                .runs
                .first()
                .expect("title line should be painted before the body text");
            assert_eq!(title_text, "Title");
            assert!(bold, "the title line must paint bold");

            let bounds = layout.bounds;
            assert!(
                title_rect.y >= bounds.y
                    && title_rect.y + title_rect.height <= bounds.y + bounds.height,
                "title row must sit inside the card's vertical bounds, got {title_rect:?} \
                 within {bounds:?}"
            );
            assert!(
                title_rect.x >= bounds.x
                    && title_rect.x + title_rect.width <= bounds.x + bounds.width,
                "title row must sit inside the card's horizontal bounds, got {title_rect:?} \
                 within {bounds:?}"
            );

            // Body text follows the title, further down inside the card —
            // not reusing the title's row.
            let (body_rect, body_text, body_bold) =
                surface.runs.get(1).expect("body text should be painted");
            assert_eq!(body_text, "Body text");
            assert!(
                !body_bold,
                "body text must not inherit the title's bold weight"
            );
            assert!(
                body_rect.y > title_rect.y + title_rect.height,
                "body text must start below the title row, got body={body_rect:?} \
                 title={title_rect:?}"
            );
        }

        /// A `Full` card sized to barely more than its one body row
        /// still paints that row: the card's vertical inset shrinks to
        /// the spare height instead of pushing the row past the bottom.
        #[test]
        fn tightly_sized_full_card_still_paints_its_body_row() {
            let tooltip = Tooltip::new(WidgetId::new("tip"), "Body text");
            let layout = TooltipLayout {
                bounds: Rect::new(10.0, 10.0, 120.0, 18.0),
                resolved_placement: ResolvedPlacement::Bottom,
            };
            let mut surface = RecordingSurface::default();
            native_surface_paint::paint(
                &tooltip,
                &layout,
                &TooltipChrome::default(),
                16.0,
                8.0,
                &mut surface,
                &Theme::default(),
                &Style::default(),
            );

            assert_eq!(surface.runs.len(), 1, "the body row must be painted");
            let (rect, text, _) = &surface.runs[0];
            assert_eq!(text, "Body text");
            assert!(rect.y > layout.bounds.y, "row sits inside the top edge");
            assert!(
                rect.y + rect.height <= layout.bounds.y + layout.bounds.height,
                "row fits inside the card, got {rect:?}"
            );
        }

        /// A titled `Full` card too short for its title plus body still
        /// starts the title below the border stroke, so no glyph ink
        /// lands on the top border row.
        #[test]
        fn tightly_sized_titled_card_keeps_title_below_border_stroke() {
            let tooltip = Tooltip::new(WidgetId::new("tip"), "Body text");
            let layout = TooltipLayout {
                bounds: Rect::new(10.0, 10.0, 120.0, 18.0),
                resolved_placement: ResolvedPlacement::Bottom,
            };
            let mut chrome = TooltipChrome::default();
            chrome.title = Some("Title".to_string());
            let mut surface = RecordingSurface::default();
            native_surface_paint::paint(
                &tooltip,
                &layout,
                &chrome,
                16.0,
                8.0,
                &mut surface,
                &Theme::default(),
                &Style::default(),
            );

            let (title_rect, title_text, _) =
                surface.runs.first().expect("title line should be painted");
            assert_eq!(title_text, "Title");
            assert!(
                title_rect.y >= layout.bounds.y + Style::default().border_width,
                "title must start below the border stroke, got {title_rect:?}"
            );
        }

        /// `Sides`/`None` chrome has no closed card for a title to sit
        /// inside — `TooltipChrome::title`'s doc says both ignore it, and
        /// that stays true under the new inside-the-card placement.
        #[test]
        fn title_still_ignored_for_sides_and_none_chrome() {
            for border in [TooltipBorder::Sides, TooltipBorder::None] {
                let tooltip = Tooltip::new(WidgetId::new("tip"), "Body text");
                let layout = TooltipLayout {
                    bounds: Rect::new(10.0, 10.0, 120.0, 16.0),
                    resolved_placement: ResolvedPlacement::Bottom,
                };
                let mut chrome = TooltipChrome::new(border);
                chrome.title = Some("Title".to_string());

                let mut surface = RecordingSurface::default();
                native_surface_paint::paint(
                    &tooltip,
                    &layout,
                    &chrome,
                    16.0,
                    8.0,
                    &mut surface,
                    &Theme::default(),
                    &Style::default(),
                );

                assert_eq!(
                    surface.runs.len(),
                    1,
                    "{border:?}: only the body text should be painted, no title line"
                );
                assert_eq!(surface.runs[0].1, "Body text");
            }
        }

        fn paint_with(border: TooltipBorder) -> RecordingSurface {
            let tooltip = Tooltip::new(WidgetId::new("tip"), "Body text");
            let layout = TooltipLayout {
                bounds: Rect::new(10.0, 10.0, 120.0, 30.0),
                resolved_placement: ResolvedPlacement::Bottom,
            };
            let mut surface = RecordingSurface::default();
            native_surface_paint::paint(
                &tooltip,
                &layout,
                &TooltipChrome::new(border),
                16.0,
                8.0,
                &mut surface,
                &Theme::default(),
                &Style::default(),
            );
            surface
        }

        /// `Full` strokes one closed rounded card around the bounds and
        /// draws no loose side bars.
        #[test]
        fn full_border_strokes_one_closed_card() {
            let surface = paint_with(TooltipBorder::Full);
            assert_eq!(
                surface.rounded_strokes,
                vec![Rect::new(10.0, 10.0, 120.0, 30.0)]
            );
            assert!(surface.lines.is_empty());
        }

        /// `Sides` draws exactly the left and right bars, full height,
        /// and no closed card.
        #[test]
        fn sides_border_draws_only_the_left_and_right_bars() {
            let surface = paint_with(TooltipBorder::Sides);
            assert!(surface.rounded_strokes.is_empty());
            assert_eq!(
                surface.lines,
                vec![
                    (Point::new(10.0, 10.0), Point::new(10.0, 40.0)),
                    (Point::new(130.0, 10.0), Point::new(130.0, 40.0)),
                ]
            );
        }

        /// `None` strokes nothing at all but still paints the text.
        #[test]
        fn none_border_strokes_nothing() {
            let surface = paint_with(TooltipBorder::None);
            assert!(surface.rounded_strokes.is_empty());
            assert!(surface.lines.is_empty());
            assert_eq!(surface.runs.len(), 1);
        }

        /// `Primitive::paint` is the shared painter with the default
        /// chrome, the surface's line height as the row pitch and its
        /// character advance as the padding.
        #[test]
        fn primitive_paint_is_the_default_chrome_card() {
            use crate::primitive::{LayoutCtx, Primitive};
            let tooltip = Tooltip::new(WidgetId::new("tip"), "Body text");
            let ctx = LayoutCtx::new(16.0, 8.0, false);
            let layout = Primitive::layout(&tooltip, &ctx, Rect::new(10.0, 10.0, 120.0, 30.0));
            let mut via_primitive = RecordingSurface::default();
            Primitive::paint(
                &tooltip,
                &layout,
                &mut via_primitive,
                &Theme::default(),
                &Style::default(),
            );
            let direct = paint_with(TooltipBorder::Full);
            assert_eq!(via_primitive.runs, direct.runs);
            assert_eq!(via_primitive.rounded_strokes, direct.rounded_strokes);
            assert_eq!(via_primitive.lines, direct.lines);
        }
    }

    // ── Cell-path paint (`cell_paint::paint`) ───────────────────────────

    #[cfg(feature = "tui")]
    mod cells {
        use super::*;
        use crate::event::Rect as QRect;
        use crate::theme::Theme;
        use crate::tui::{draw_tooltip, draw_tooltip_with_chrome};
        use crate::types::StyledSpan;
        use cell_paint::painted_bounds;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        fn make_layout(x: f32, y: f32, w: f32, h: f32) -> TooltipLayout {
            TooltipLayout {
                bounds: QRect::new(x, y, w, h),
                resolved_placement: ResolvedPlacement::Bottom,
            }
        }

        fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
            buf[(x, y)].symbol().chars().next().unwrap_or(' ')
        }

        /// The zone a backend registers for a
        /// tooltip must match the cells `draw_tooltip` actually painted, not
        /// the raw float layout bounds — otherwise a structural-parity
        /// observer sees a surface offset from what's really on screen.
        #[test]
        fn painted_bounds_rounds_like_draw_tooltip_does() {
            let layout = make_layout(0.4, 1.6, 9.6, 2.5);
            let painted = painted_bounds(&layout);
            assert_eq!(painted, QRect::new(0.0, 2.0, 10.0, 3.0));
        }

        #[test]
        fn paints_side_borders_and_plain_text() {
            let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
            let tt = Tooltip {
                id: WidgetId::new("hover"),
                text: "hello".into(),
                styled_lines: None,
                placement: TooltipPlacement::Bottom,
                fg: None,
                bg: None,
            };
            let layout = make_layout(0.0, 0.0, 10.0, 1.0);
            draw_tooltip(&mut buf, &tt, &layout, &Theme::default());

            // Borders at col 0 and col 9.
            assert_eq!(cell_char(&buf, 0, 0), '│');
            assert_eq!(cell_char(&buf, 9, 0), '│');
            // Text starts at col 2.
            let row: String = (2..7).map(|x| cell_char(&buf, x, 0)).collect();
            assert_eq!(row, "hello");
        }

        /// Once there's room for both border rows (`height >= 3`),
        /// the box must be closed on every side — not just left/right — so a
        /// structural-parity observer sees the same chrome shape GTK has
        /// always drawn.
        #[test]
        fn paints_a_full_box_when_height_allows_it() {
            let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
            let tt = Tooltip {
                id: WidgetId::new("hover"),
                text: "hello".into(),
                styled_lines: None,
                placement: TooltipPlacement::Bottom,
                fg: None,
                bg: None,
            };
            let layout = make_layout(0.0, 0.0, 10.0, 3.0);
            draw_tooltip(&mut buf, &tt, &layout, &Theme::default());

            // Top row: corners + a solid horizontal rule.
            assert_eq!(cell_char(&buf, 0, 0), '┌');
            assert_eq!(cell_char(&buf, 9, 0), '┐');
            assert_eq!(cell_char(&buf, 4, 0), '─');
            // Bottom row: corners + a solid horizontal rule.
            assert_eq!(cell_char(&buf, 0, 2), '└');
            assert_eq!(cell_char(&buf, 9, 2), '┘');
            assert_eq!(cell_char(&buf, 4, 2), '─');
            // Content moved down one row, side borders unchanged.
            assert_eq!(cell_char(&buf, 0, 1), '│');
            assert_eq!(cell_char(&buf, 9, 1), '│');
            let row: String = (2..7).map(|x| cell_char(&buf, x, 1)).collect();
            assert_eq!(row, "hello");
        }

        #[test]
        fn styled_lines_paint_each_row() {
            let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
            let tt = Tooltip {
                id: WidgetId::new("sig"),
                text: String::new(),
                styled_lines: Some(vec![
                    StyledText {
                        spans: vec![StyledSpan::plain("line1")],
                    },
                    StyledText {
                        spans: vec![StyledSpan::plain("line2")],
                    },
                ]),
                placement: TooltipPlacement::Bottom,
                fg: None,
                bg: None,
            };
            let layout = make_layout(0.0, 0.0, 12.0, 2.0);
            draw_tooltip(&mut buf, &tt, &layout, &Theme::default());

            let r0: String = (2..7).map(|x| cell_char(&buf, x, 0)).collect();
            let r1: String = (2..7).map(|x| cell_char(&buf, x, 1)).collect();
            assert_eq!(r0, "line1");
            assert_eq!(r1, "line2");
        }

        #[test]
        fn per_tooltip_bg_overrides_theme() {
            let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
            let tt = Tooltip {
                id: WidgetId::new("hover"),
                text: "x".into(),
                styled_lines: None,
                placement: TooltipPlacement::Bottom,
                fg: None,
                bg: Some(Color::rgb(100, 0, 0)),
            };
            let layout = make_layout(0.0, 0.0, 10.0, 1.0);
            draw_tooltip(&mut buf, &tt, &layout, &Theme::default());
            // Cell 2 should have bg = (100, 0, 0).
            let bg = buf[(2u16, 0u16)].bg;
            assert_eq!(bg, ratatui::style::Color::Rgb(100, 0, 0));
        }

        #[test]
        fn zero_size_is_a_no_op() {
            let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
            let tt = Tooltip {
                id: WidgetId::new("hover"),
                text: "x".into(),
                styled_lines: None,
                placement: TooltipPlacement::Bottom,
                fg: None,
                bg: None,
            };
            let layout = make_layout(0.0, 0.0, 0.0, 1.0);
            draw_tooltip(&mut buf, &tt, &layout, &Theme::default());
            assert_eq!(cell_char(&buf, 0, 0), ' ');
        }

        fn tooltip_with() -> Tooltip {
            Tooltip::new(WidgetId::new("hover"), "hi")
        }

        /// `make_layout` paired with the [`TooltipChrome`] a test
        /// wants — the in-crate equivalent of `tooltip.layout(...)` plus a
        /// `TooltipChrome::new(..)` sidecar with `title` set directly.
        fn layout_with(
            w: f32,
            h: f32,
            border: TooltipBorder,
            title: Option<&str>,
        ) -> (TooltipLayout, TooltipChrome) {
            let mut chrome = TooltipChrome::new(border);
            chrome.title = title.map(str::to_string);
            (make_layout(0.0, 0.0, w, h), chrome)
        }

        /// `Sides` must never draw top/bottom rules, even at a height that
        /// would give `Full` plenty of room for a closed box — the whole
        /// point of separating the two is that a consumer can *ask* for the
        /// narrow look regardless of how tall its content is.
        #[test]
        fn sides_border_never_closes_even_when_height_allows_it() {
            let mut buf = Buffer::empty(Rect::new(0, 0, 30, 8));
            // 5 lines so every one of the 5 rows the layout reserves actually
            // gets painted (a row with no corresponding text line is left
            // untouched by `draw_tooltip`, which would make an empty row's
            // "no border here" look identical to a genuinely-suppressed one).
            let mut tt = tooltip_with();
            tt.text = "hi\nhi\nhi\nhi\nhi".into();
            let (layout, chrome) = layout_with(10.0, 5.0, TooltipBorder::Sides, None);
            draw_tooltip_with_chrome(&mut buf, &tt, &layout, &chrome, &Theme::default());

            for row in 0..5u16 {
                assert_eq!(
                    cell_char(&buf, 0, row),
                    '│',
                    "row {row}: Sides must paint the left bar on every row"
                );
                assert_eq!(
                    cell_char(&buf, 9, row),
                    '│',
                    "row {row}: Sides must paint the right bar on every row"
                );
            }
            // No corners anywhere — a top/bottom rule would introduce one.
            for row in 0..5u16 {
                for col in 0..10u16 {
                    let ch = cell_char(&buf, col, row);
                    assert!(
                        !['┌', '┐', '└', '┘', '─'].contains(&ch),
                        "Sides must never paint box-drawing corner/rule glyphs \
                         (found {ch:?} at ({col}, {row}))"
                    );
                }
            }
        }

        /// `None` paints no border chrome at all — not even the side bars
        /// `Sides` (and `Full`'s too-short fallback) always paint.
        #[test]
        fn none_border_paints_no_chrome() {
            let mut buf = Buffer::empty(Rect::new(0, 0, 30, 8));
            let tt = tooltip_with();
            let (layout, chrome) = layout_with(10.0, 3.0, TooltipBorder::None, None);
            draw_tooltip_with_chrome(&mut buf, &tt, &layout, &chrome, &Theme::default());

            for row in 0..3u16 {
                for col in 0..10u16 {
                    let ch = cell_char(&buf, col, row);
                    assert!(
                        !['┌', '┐', '└', '┘', '─', '│'].contains(&ch),
                        "None must paint no border glyph at all (found {ch:?} at ({col}, {row}))"
                    );
                }
            }
            // Content starts one column earlier than `Full`/`Sides` (padding
            // only, no border column to clear first).
            let row: String = (1..3).map(|x| cell_char(&buf, x, 0)).collect();
            assert_eq!(row, "hi");
        }

        /// An optional title is centred into the top border row
        /// when `Full` actually closes the box.
        #[test]
        fn full_border_centers_title_in_top_row() {
            let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
            let tt = tooltip_with();
            let (layout, chrome) = layout_with(12.0, 3.0, TooltipBorder::default(), Some("Hi"));
            draw_tooltip_with_chrome(&mut buf, &tt, &layout, &chrome, &Theme::default());

            // Interior columns are 1..=10 (corners at 0 and 11); " Hi " (4
            // chars) centred among 10 interior columns starts at col 1 + 3 = 4.
            let top_row: String = (0..12).map(|x| cell_char(&buf, x, 0)).collect();
            assert_eq!(top_row, "┌─── Hi ───┐");
            // Bottom row carries no title — plain rule.
            let bottom_row: String = (0..12).map(|x| cell_char(&buf, x, 2)).collect();
            assert_eq!(bottom_row, "└──────────┘");
        }

        /// A title that doesn't fit the interior width is dropped rather than
        /// clipped mid-glyph.
        #[test]
        fn oversized_title_is_dropped_not_truncated() {
            let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
            let tt = tooltip_with();
            let (layout, chrome) = layout_with(
                12.0,
                3.0,
                TooltipBorder::default(),
                Some("This title is far too long for the box"),
            );
            draw_tooltip_with_chrome(&mut buf, &tt, &layout, &chrome, &Theme::default());

            let top_row: String = (0..12).map(|x| cell_char(&buf, x, 0)).collect();
            assert_eq!(
                top_row, "┌──────────┐",
                "an oversized title must fall back to a plain rule, not a clipped label"
            );
        }

        /// `title` has no top rule to embed into on `Sides` or `None`, so it
        /// must not leak into the content area either.
        #[test]
        fn title_is_ignored_when_border_is_not_full() {
            for border in [TooltipBorder::Sides, TooltipBorder::None] {
                let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
                let tt = tooltip_with();
                let (layout, chrome) = layout_with(12.0, 3.0, border, Some("Hi"));
                draw_tooltip_with_chrome(&mut buf, &tt, &layout, &chrome, &Theme::default());

                let screen: String = (0..3)
                    .flat_map(|row| (0..12).map(move |col| (col, row)))
                    .map(|(col, row)| cell_char(&buf, col, row))
                    .collect();
                assert!(
                    !screen.contains("Hi"),
                    "{border:?}: title must not appear anywhere when there's no top rule to \
                     embed it in (screen contents: {screen:?})"
                );
            }
        }

        /// `Full`'s degrade-to-`Sides` fallback (too short for a box) must
        /// also drop the title — there's no top row to put it in once the
        /// box doesn't close.
        #[test]
        fn full_border_drops_title_when_too_short_to_close() {
            let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
            let tt = tooltip_with();
            let (layout, chrome) = layout_with(12.0, 1.0, TooltipBorder::default(), Some("Hi"));
            draw_tooltip_with_chrome(&mut buf, &tt, &layout, &chrome, &Theme::default());

            assert_eq!(cell_char(&buf, 0, 0), '│');
            let row: String = (0..12).map(|x| cell_char(&buf, x, 0)).collect();
            assert!(
                !row.contains("Hi"),
                "a too-short box has no top row to embed the title in: {row:?}"
            );
        }

        /// `Primitive::paint_cells` paints exactly what the default-chrome
        /// entry point paints.
        #[test]
        fn primitive_paint_cells_is_the_default_chrome_box() {
            use crate::primitive::Primitive;
            let tt = tooltip_with();
            let layout = make_layout(1.0, 1.0, 10.0, 3.0);
            let mut via_primitive = Buffer::empty(Rect::new(0, 0, 20, 6));
            Primitive::paint_cells(&tt, &layout, &mut via_primitive, &Theme::default());
            let mut direct = Buffer::empty(Rect::new(0, 0, 20, 6));
            draw_tooltip(&mut direct, &tt, &layout, &Theme::default());
            assert_eq!(via_primitive, direct);
        }

        /// A box sized by `Primitive::measure` shows every content row
        /// inside a closed border.
        #[test]
        fn measured_box_shows_every_row() {
            use crate::primitive::{Primitive, Size};
            let tt = Tooltip::new(WidgetId::new("tip"), "one\ntwo\nthree");
            let size = Primitive::measure(&tt, &super::cell_ctx(), Size::new(40.0, 10.0));
            let rect = QRect::new(0.0, 0.0, size.width.unwrap(), size.height.unwrap());
            let layout = Primitive::layout(&tt, &super::cell_ctx(), rect);
            let mut buf = Buffer::empty(Rect::new(0, 0, 40, 10));
            Primitive::paint_cells(&tt, &layout, &mut buf, &Theme::default());

            let row = |y: u16| -> String { (0..8).map(|x| cell_char(&buf, x, y)).collect() };
            assert_eq!(row(0), "┌──────┐");
            assert_eq!(row(1), "│ one  │");
            assert_eq!(row(2), "│ two  │");
            assert_eq!(row(3), "│ three│");
            assert_eq!(row(4), "└──────┘");
        }
    }
}
