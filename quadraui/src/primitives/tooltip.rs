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
//! ## Downstream impact (#541)
//!
//! `border: TooltipBorder` and `title: Option<String>` are **new required
//! fields on a public struct consumers construct**, so per rule 8 of
//! `quadraui/docs/PRIMITIVE_RULES.md` this is a breaking change: every
//! *exhaustive* `Tooltip { .. }` literal in a consumer stops compiling the
//! moment this lands on `develop`. There is no Rust shim that keeps an
//! exhaustive literal compiling across an added field, so the break itself
//! cannot be deprecated away — only made small and made final.
//!
//! **Made small.** [`Tooltip`] now implements [`Default`], so each affected
//! site migrates by deleting the fields it was leaving at the default and
//! adding one line:
//!
//! ```
//! # use quadraui::{Tooltip, TooltipPlacement, WidgetId};
//! let tip = Tooltip {
//!     id: WidgetId::new("hover"),
//!     text: "Hover hint".to_string(),
//!     placement: TooltipPlacement::Top,
//!     ..Default::default()
//! };
//! assert_eq!(tip.title, None);
//! ```
//!
//! **Made final.** That same `..Default::default()` spread is what makes
//! the *next* added field a non-event: a literal written this way keeps
//! compiling when the struct grows. [`Tooltip::new`] plus the `with_*`
//! builder methods are the equivalent path for sites that would rather not
//! name fields at all. This is why `Tooltip` stays a plain struct rather
//! than gaining `#[non_exhaustive]`: `#[non_exhaustive]` would forbid
//! struct-literal *and* functional-update syntax outside this crate
//! entirely, forcing every consumer through the builder forever and
//! deleting the cheap one-line escape hatch above, for no protection that
//! `Default` + the spread does not already buy. ([`TooltipBorder`] *is*
//! `#[non_exhaustive]` — different calculus: it is brand new this PR, so
//! nothing matches on it yet and marking it costs no consumer anything.)
//!
//! `vimcode` constructs `Tooltip` exhaustively at 7 call sites, all of
//! which need that one-line migration:
//!
//! - `src/tui_main/panels.rs:810`
//! - `src/render.rs:1151`, `src/render.rs:1490`, `src/render.rs:4285`
//! - `src/gtk/draw.rs:1378`, `src/gtk/draw.rs:1667`, `src/gtk/draw.rs:1765`
//!
//! Per rule 8's two-PR protocol that needs a companion `vimcode` migration
//! issue/PR opened and linked before or alongside merge — a cross-repo
//! `gh` action outside a Work dispatch's tool access, so it is the job of
//! whoever lands this (see the PR discussion for #541). `coord-tui` pins a
//! fixed git rev and so is unaffected until someone bumps it.
//!
//! In-tree, every construction site is already migrated: the sealed
//! acceptance slices under `tests/acceptance/` and the `quadraui/tests/`
//! integration crates build through [`Tooltip::new`] or the spread above,
//! so `cargo test --features tui` is green on this branch.

use crate::event::Rect;
use crate::types::{Color, Modifiers, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a tooltip.
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
    /// Border chrome the consumer wants drawn around the box. `None` (the
    /// Rust default, not [`TooltipBorder::None`]) is
    /// [`TooltipBorder::Full`] — see that variant's docs for why that,
    /// not [`TooltipBorder::Sides`], is the behaviour-preserving default
    /// (#541).
    #[serde(default)]
    pub border: TooltipBorder,
    /// Optional title rendered centred into the top border row. Only
    /// meaningful when `border` is [`TooltipBorder::Full`] — `Sides` and
    /// `None` have no top rule to embed it in, so backends ignore it in
    /// those modes rather than falling back to a content row (#541 ask 2).
    #[serde(default)]
    pub title: Option<String>,
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
/// `Rounded` corner style) is additive instead of another breaking change
/// like the one adding this whole type was (see the module doc's
/// *Downstream impact* section).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TooltipBorder {
    /// Vertical bars on the left/right edge only — no top or bottom rule,
    /// regardless of box height. The pre-#542 TUI look, now available on
    /// every backend by explicit request. `title` is not rendered in this
    /// mode (there is no top rule to embed it in).
    Sides,
    /// A closed box on all four sides. The default (see the note on
    /// [`Tooltip::border`]): GTK and macOS have always stroked a full
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

/// Events a `Tooltip` emits. Tooltips are non-interactive; events exist
/// for parity with other primitives but rarely fire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TooltipEvent {
    KeyPressed { key: String, modifiers: Modifiers },
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
#[derive(Debug, Clone, Copy, PartialEq)]
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

/// Every field but `id`/`text` at its behaviour-preserving default
/// (`styled_lines: None`, `placement: Bottom`, `border: Full`, `title:
/// None`, `bg: None`, `fg: None`), with `id`/`text` empty.
///
/// This exists so consumers can write `Tooltip { id, text,
/// ..Default::default() }` and keep compiling when the struct grows another
/// field — see the module doc's *Downstream impact* section. An
/// all-defaults `Tooltip` has an empty [`WidgetId`], which is only useful
/// as the tail of such a spread; use [`Tooltip::new`] to get an identified
/// one.
impl Default for Tooltip {
    fn default() -> Self {
        Self::new(WidgetId::new(String::new()), String::new())
    }
}

impl Tooltip {
    /// Construct a `Tooltip` with just the two fields every consumer must
    /// supply — `id` and `text` — and every other field at its
    /// behaviour-preserving default (`styled_lines: None`, `placement:
    /// Bottom`, `border: Full`, `title: None`, `bg: None`, `fg: None`).
    ///
    /// This plus the `with_*` builder methods below is one of the two
    /// field-addition-proof ways to build a `Tooltip`; the other is
    /// `Tooltip { id, text, ..Default::default() }`. Prefer either over an
    /// *exhaustive* `Tooltip { .. }` literal, which is re-broken by the
    /// next added field exactly the way `border`/`title` broke every
    /// exhaustive literal in #541 (module doc, *Downstream impact*).
    pub fn new(id: WidgetId, text: impl Into<String>) -> Self {
        Self {
            id,
            text: text.into(),
            styled_lines: None,
            placement: TooltipPlacement::default(),
            border: TooltipBorder::default(),
            title: None,
            bg: None,
            fg: None,
        }
    }

    /// Set `styled_lines` (per-span-coloured multi-line content, in place
    /// of `text`).
    pub fn with_styled_lines(mut self, styled_lines: Vec<StyledText>) -> Self {
        self.styled_lines = Some(styled_lines);
        self
    }

    /// Set the preferred placement relative to the anchor.
    pub fn with_placement(mut self, placement: TooltipPlacement) -> Self {
        self.placement = placement;
        self
    }

    /// Set the border chrome (#541).
    pub fn with_border(mut self, border: TooltipBorder) -> Self {
        self.border = border;
        self
    }

    /// Set a title, centred into the top border row when `border` is
    /// [`TooltipBorder::Full`] (#541). Ignored by `Sides` and `None`.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Override the background colour (theme default otherwise).
    pub fn with_bg(mut self, bg: Color) -> Self {
        self.bg = Some(bg);
        self
    }

    /// Override the foreground colour (theme default otherwise).
    pub fn with_fg(mut self, fg: Color) -> Self {
        self.fg = Some(fg);
        self
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
