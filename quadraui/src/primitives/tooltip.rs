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
    use super::{Tooltip, TooltipBorder, TooltipChrome, TooltipLayout};
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
        // *horizontal* body inset, see `text_padding_x` below). `Sides`
        // and `None` have no top rule to clear, so content starts flush
        // with the box's top edge — callers following the
        // `TooltipMeasure` contract size those two variants to exactly the
        // content rows they hold, with no border row to spare.
        const CARD_VPAD: f32 = 4.0;
        let mut text_top = match chrome.border {
            TooltipBorder::Full => bounds.y + CARD_VPAD,
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
        if matches!(chrome.border, TooltipBorder::Full) {
            if let Some(title) = chrome
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                let (title_w, title_h) = surface.surface_measure_text_styled(title, true);
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
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
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
                _from: Point,
                _to: Point,
                _color: Color,
                _stroke_width: f32,
            ) {
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
    }
}
