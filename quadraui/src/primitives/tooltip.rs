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
//! ## Border/title vocabulary is a *sidecar*, not a field on either struct (#541)
//!
//! [`TooltipBorder`] and an optional title are per-tooltip chrome a
//! consumer can now ask for (`Sides` bars-only, `Full` closed box, `None`
//! no chrome at all; an optional title centred into `Full`'s top border
//! row). They live together in [`TooltipChrome`], a **brand-new type**
//! passed *alongside* the tooltip and its layout to
//! [`crate::Backend::draw_tooltip_with_chrome`] (and the per-backend
//! `draw_tooltip_with_chrome` rasterisers).
//!
//! They are deliberately **not** fields on [`Tooltip`] and **not** fields
//! on [`TooltipLayout`]. Both of those are plain, non-`#[non_exhaustive]`,
//! all-`pub`-field structs, and both are constructed with *exhaustive*
//! literals today — `Tooltip { .. }` by the sealed acceptance slices under
//! `tests/acceptance/ms-11/` and by seven `vimcode` call sites, and
//! `TooltipLayout { bounds, resolved_placement }` by hand at
//! `vimcode`'s `src/tui_main/panels.rs` (that popup centres itself over an
//! area rather than anchoring to an element, so it cannot use
//! [`Tooltip::layout`] and builds the layout value directly — its public
//! fields exist for exactly that). Adding a required field to *either*
//! struct is an `error[E0063]: missing fields` break for every one of
//! those literals the instant it lands on `develop`, and Rust offers no
//! shim that keeps an exhaustive literal compiling across an added field:
//!
//! - `Default` + `..Default::default()` only helps literals that already
//!   spread, which these don't.
//! - `#[non_exhaustive]` applied retroactively swaps `E0063` for
//!   `E0639: cannot construct non-exhaustive struct outside its crate` —
//!   strictly worse for `panels.rs`, which *needs* to construct one.
//!
//! Threading the vocabulary through a separate value sidesteps the break
//! entirely. Nothing about `Tooltip`'s or `TooltipLayout`'s field set
//! changes in #541, so every existing exhaustive literal of either — in
//! tree, in the sealed acceptance slices, and downstream — keeps compiling
//! untouched, and the new capability is reached by calling a new method
//! instead of by filling in a new field. Per rule 8 of
//! `quadraui/docs/PRIMITIVE_RULES.md` this makes #541 a purely additive
//! change with no downstream migration to schedule.
//!
//! [`crate::Backend::draw_tooltip`] keeps its exact signature and
//! behaviour (it renders `TooltipChrome::default()`, i.e. the
//! [`TooltipBorder::Full`] box every backend already drew), so existing
//! `Backend` implementors and callers are untouched too;
//! `draw_tooltip_with_chrome` is an added trait method with a default body
//! that delegates to it.
//!
//! Measured per rule 8's *Measure before you cut* (grepped across both
//! path-dep consumers, `~/src/vimcode/src` and
//! `~/src/claude-coordinator/tui/src`): 7 exhaustive `Tooltip { .. }`
//! literals in `vimcode` (`src/render.rs:1151,1490,4285`;
//! `src/gtk/draw.rs:1378,1667,1765`; `src/tui_main/panels.rs:810`), 1
//! exhaustive `TooltipLayout { .. }` literal (`panels.rs:818`), and no
//! tooltip hits at all in `coord-tui`. Every one of them still compiles
//! against this change — nothing to migrate, so no companion consumer PR
//! and no `#[deprecated]` shim are required.
//!
//! ## `with_*` builder removal (issue #824, then #1109)
//!
//! `Tooltip`'s `with_styled_lines` / `with_placement` / `with_bg` /
//! `with_fg` and `TooltipChrome`'s `with_border` / `with_title` builders
//! were pure convenience sugar around a field write, not something that
//! guarded an invariant — every one of those fields is already `pub`.
//! #824 deprecated them; #1109 removed them (zero uses in coord-tui's
//! `main` and vimcode's `develop`). Set the field directly instead —
//! `TooltipChrome` is `#[non_exhaustive]` + `Default` already, so it *is*
//! the options-struct shape this issue asks for, and a future chrome/
//! tooltip field is additive either way. `Tooltip` itself stays a plain,
//! non-`#[non_exhaustive]` struct per the exhaustive-literal reasoning
//! above — that part of #824's "adding a field must not break" goal
//! already lives on `TooltipChrome`, which is why new tooltip knobs have
//! gone there (`#541`) rather than onto `Tooltip` since.
//!
//! Usage:
//!
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
/// let anchor = Rect::new(0.0, 0.0, 10.0, 1.0);
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

/// Border chrome vocabulary for [`Tooltip`] (#541).
///
/// Before this existed, each backend hardcoded its own answer — TUI drew
/// `│` side bars only, GTK and macOS always stroked a full 4-sided box —
/// so a consumer had no way to ask for one or the other; a raw-drawing
/// popup that migrated to `Backend::draw_tooltip` (JDonaghy/vimcode#635)
/// silently lost its top/bottom border and title because there was no
/// field to carry the request. This type is that field's vocabulary.
///
/// `#[non_exhaustive]`: brand new this PR, so marking it costs no
/// consumer anything today, and it means a future fourth variant (e.g. a
/// `Rounded` corner style) is additive instead of a breaking change.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TooltipBorder {
    /// Vertical bars on the left/right edge only — no top or bottom rule,
    /// regardless of box height. The pre-#542 TUI look, now available on
    /// every backend by explicit request. `title` is not rendered in this
    /// mode (there is no top rule to embed it in).
    Sides,
    /// A closed box on all four sides. The default (see the note on
    /// [`TooltipLayout::border`]): GTK and macOS have always stroked a full
    /// rectangle here, and TUI has done the same since #542 whenever the
    /// measured box leaves room for both border rows (`height >= 3` and
    /// `width >= 2`); below that TUI falls back to `Sides` chrome for
    /// this variant specifically, so a tooltip too short for a box still
    /// shows *something* — that fallback is a rendering detail of `Full`,
    /// not a separate mode a consumer selects.
    ///
    /// Carries `title`, centred into the top border row, when set.
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
/// # Contract (#542)
///
/// `height` is the whole tooltip box, border chrome included — not just
/// the content. `Tooltip::layout` passes it straight through to
/// [`TooltipLayout::bounds`], and that is what a backend's `draw_tooltip`
/// paints into. Concretely, on TUI:
///
/// - When `height >= 3` (and `width >= 2`), [`crate::tui::draw_tooltip`]
///   strokes a full 4-sided box — one row each for the top and bottom
///   border — leaving only `height - 2` rows for content. GTK has always
///   stroked a full box the same way, but pays for it in sub-cell pixels
///   rather than whole rows, so it needs no equivalent padding.
/// - Below that (`height < 3`), TUI falls back to side-bars-only chrome
///   (`│` on the first/last column, no top/bottom rule) and all `height`
///   rows are usable for content.
///
/// **Callers that want *N* content lines visible on TUI once a bordered
/// box is drawn must pass `height >= N + 2`**, not `N`. This is a
/// behaviour change from before #542, when TUI drew no vertical chrome at
/// all and `height` meant exactly "content rows" on every backend; a
/// caller written against that older contract that still passes a bare
/// content-line count will silently lose its last two lines once its
/// tooltip is 3+ rows tall. See the #542 review discussion for the
/// concrete downstream call sites this bit (`vimcode`'s hover-popup and
/// diff-peek tooltip builders) — those need a matching `+ 2` migration.
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
/// directly with an exhaustive literal. #541's border/title vocabulary
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
            TooltipHit::Body(id.clone())
        } else {
            TooltipHit::Empty
        }
    }
}

/// Per-tooltip chrome request (#541): which border a backend should
/// stroke, and an optional title to embed in it.
///
/// Passed alongside a [`Tooltip`] + [`TooltipLayout`] to
/// [`crate::Backend::draw_tooltip_with_chrome`]. It is a separate value
/// rather than fields on either of those structs so that #541 adds no
/// required field to a publicly-literal-constructible type — see the
/// module doc for the full reasoning.
///
/// [`TooltipChrome::default()`] is `Full` with no title: exactly what
/// every backend drew unconditionally before this type existed, which is
/// why [`crate::Backend::draw_tooltip`] can keep its old signature and
/// simply render the default.
///
/// `#[non_exhaustive]`: brand new in this change, so marking it costs no
/// consumer anything today (nobody has an exhaustive literal of it yet),
/// and it means future chrome knobs — a corner radius, a border colour
/// override — are additive rather than the very breaking change this type
/// exists to avoid. Construct with [`TooltipChrome::new`] /
/// [`TooltipChrome::default`], then set `border` / `title` directly —
/// both fields are `pub` and the type is already `#[non_exhaustive]` +
/// `Default`, so it *is* the options struct (quadraui#824); the
/// `with_border` / `with_title` builders that used to be the only way to
/// reach those fields were removed in issue #1109.
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
    /// # Border/title (#541)
    ///
    /// The returned [`TooltipLayout`] carries geometry only. Border chrome
    /// and an optional title are requested separately, via a
    /// [`TooltipChrome`] value passed alongside this layout to
    /// [`crate::Backend::draw_tooltip_with_chrome`]; drawing through plain
    /// [`crate::Backend::draw_tooltip`] uses `TooltipChrome::default()`
    /// ([`TooltipBorder::Full`], no title) — the behaviour every backend
    /// used unconditionally before #541 introduced a choice. See the module
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

        // Compute preferred x/y for each possible placement.
        let candidate = |p: TooltipPlacement| -> (f32, f32) {
            match p {
                TooltipPlacement::Top => {
                    (anchor.x + (anchor.width - vw) * 0.5, anchor.y - margin - vh)
                }
                TooltipPlacement::Bottom => (
                    anchor.x + (anchor.width - vw) * 0.5,
                    anchor.y + anchor.height + margin,
                ),
                TooltipPlacement::Left => (
                    anchor.x - margin - vw,
                    anchor.y + (anchor.height - vh) * 0.5,
                ),
                TooltipPlacement::Right => (
                    anchor.x + anchor.width + margin,
                    anchor.y + (anchor.height - vh) * 0.5,
                ),
            }
        };

        let fits = |x: f32, y: f32| -> bool {
            x >= viewport.x
                && x + vw <= viewport.x + viewport.width
                && y >= viewport.y
                && y + vh <= viewport.y + viewport.height
        };

        // Try preferred, then opposite, then clamp.
        let opposite = match self.placement {
            TooltipPlacement::Top => TooltipPlacement::Bottom,
            TooltipPlacement::Bottom => TooltipPlacement::Top,
            TooltipPlacement::Left => TooltipPlacement::Right,
            TooltipPlacement::Right => TooltipPlacement::Left,
        };

        let (x, y, resolved) = {
            let (px, py) = candidate(self.placement);
            if fits(px, py) {
                (px, py, self.placement)
            } else {
                let (ox, oy) = candidate(opposite);
                if fits(ox, oy) {
                    (ox, oy, opposite)
                } else {
                    // Fall back to preferred, clamped to viewport. Guard
                    // against `vw > viewport.width` / `vh > viewport.height`
                    // — without `.max(viewport.x)` the clamp's max would be
                    // less than its min and `f32::clamp` panics. When the
                    // tooltip is too big to fit, pin it to the viewport
                    // edge and let it overflow on the far side rather than
                    // crash; the consumer is responsible for choosing a
                    // sensible width if overflow is undesirable.
                    let max_x = (viewport.x + viewport.width - vw).max(viewport.x);
                    let max_y = (viewport.y + viewport.height - vh).max(viewport.y);
                    let cx = px.clamp(viewport.x, max_x);
                    let cy = py.clamp(viewport.y, max_y);
                    (cx, cy, self.placement)
                }
            }
        };

        let resolved_placement = match resolved {
            TooltipPlacement::Top => ResolvedPlacement::Top,
            TooltipPlacement::Bottom => ResolvedPlacement::Bottom,
            TooltipPlacement::Left => ResolvedPlacement::Left,
            TooltipPlacement::Right => ResolvedPlacement::Right,
        };

        TooltipLayout {
            bounds: Rect::new(x, y, vw, vh),
            resolved_placement,
        }
    }
}

// ── PaintSurface Phase 4 slice 4/8 (#1077) ─────────────────────────────────
//
// `paint` below is the one shared paint implementation, written against
// [`crate::paint_surface::PaintSurface`] instead of any one backend's
// API — see `crate::primitives::context_menu::native_surface_paint` for
// the same pattern applied one primitive earlier in this slice (#1077,
// part 1/4).
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
//   own, so those two were dropped there too); GTK and macOS both
//   ignored all three — GTK explicitly cleared Pango attributes before
//   drawing each span, and macOS's `draw_text`/`measure_text` never took
//   a style parameter at all. `paint` calls
//   [`crate::paint_surface::PaintSurface::surface_draw_text_run_styled`]
//   /`surface_measure_text_styled` for every backend now: Windows keeps
//   its bold rendering exactly as before (dropping italic/underline, per
//   [`crate::paint_surface::PaintSurface::surface_draw_text_run_styled`]'s
//   own documented per-backend override survey); GTK gains real
//   bold/italic/underline rendering it never had; macOS's adapter takes
//   the verb's default (ignore style, forward to the plain measure/draw)
//   so its rendering is unchanged. Plain `tooltip.text` (no
//   `styled_lines`) is unaffected on every backend — it never carried
//   per-run style.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{Tooltip, TooltipBorder, TooltipChrome, TooltipLayout};
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;
    use crate::{Point, Rect};

    /// Paint a [`Tooltip`] at its resolved `tooltip_layout` onto
    /// `surface`, with the border and optional title requested by
    /// `chrome` (#541).
    ///
    /// `line_height` / `padding_x` are in `surface`-native units
    /// (character cells for TUI-shaped callers, pixels for every pixel
    /// backend — though TUI itself never calls this, see the module
    /// doc's *Why TUI stays out* on [`PaintSurface`]). `padding_x` is
    /// halved when `chrome.border` is [`TooltipBorder::None`], since
    /// there is no border column to clear first.
    pub(crate) fn paint(
        tooltip: &Tooltip,
        tooltip_layout: &TooltipLayout,
        chrome: &TooltipChrome,
        line_height: f32,
        padding_x: f32,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
    ) {
        let bounds = tooltip_layout.bounds;
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }

        let bg = tooltip.bg.unwrap_or(theme.hover_bg);
        let fg = tooltip.fg.unwrap_or(theme.hover_fg);
        let border = theme.hover_border;

        surface.surface_fill_rect(bounds, bg);

        // Content normally starts 2 units below the top edge; a title
        // pushes that down further, since its real font height
        // (`title_h`) is typically much taller than the 1-unit border
        // line it's centred on — without this, a title would visually
        // collide with the first content row instead of sitting in its
        // own space above it.
        let mut text_top = bounds.y + 2.0;

        match chrome.border {
            TooltipBorder::Full => {
                surface.surface_stroke_rect(bounds, border, 1.0);

                if let Some(title) = chrome
                    .title
                    .as_deref()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                {
                    let (title_w, title_h) = surface.surface_measure_text(title);
                    let pad = 4.0;
                    let title_x = bounds.x + ((bounds.width - title_w) / 2.0).max(0.0);
                    let title_y = bounds.y - title_h / 2.0;

                    // Punch a background-coloured gap through the border
                    // stroke so the title reads as embedded in the top
                    // rule, not a content row sitting on top of it.
                    surface.surface_fill_rect(
                        Rect::new(title_x - pad, title_y, title_w + pad * 2.0, title_h),
                        bg,
                    );
                    surface.surface_draw_text_run(
                        Rect::new(title_x, title_y, title_w.max(1.0), title_h.max(1.0)),
                        title,
                        fg,
                    );

                    text_top = text_top.max(title_y + title_h + 2.0);
                }
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

    /// #541: the border/title vocabulary is a separate [`TooltipChrome`]
    /// value passed alongside the tooltip and its layout — *not* a field
    /// on `Tooltip` or on `TooltipLayout`, both of which consumers
    /// construct with exhaustive literals (see `primitives::tooltip`'s
    /// module doc). `TooltipChrome::default()` matches what every backend
    /// drew unconditionally before #541 introduced a choice.
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

    /// #541 rule-8 guard: `TooltipLayout` must stay constructible from a
    /// bare exhaustive literal, because downstream consumers that
    /// position a popup themselves (rather than anchoring it via
    /// `Tooltip::layout`) build one by hand. If a future change adds a
    /// required field here, this test stops compiling — which is the
    /// point: that would be an `E0063` break for every such call site.
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
        // Regression for #213: when content width exceeds viewport width
        // the legacy clamp produced max < min and `f32::clamp` panicked.
        // The fix pins the tooltip to the viewport edge.
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
}
