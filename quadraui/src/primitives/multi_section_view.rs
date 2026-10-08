//! `MultiSectionView` primitive: vertically stacked, individually sized,
//! collapsible sections — each containing its own scrollable body widget.
//!
//! Used for VSCode-style side panels (Explorer's Open Editors / Folder /
//! Outline / Timeline), Source Control panels, Debug sidebars, k8s resource
//! browsers, Postman collection sidebars — anything that's "N stacked
//! sections, each its own little scrollable list, with chrome on top."
//!
//! See `quadraui/docs/decisions/DECISIONS.md` D-003 for the design pass that
//! produced this primitive.
//!
//! # Composition vs subsumption
//!
//! `MultiSectionView` does NOT reimplement tree / list / form painting.
//! Each section's `body` is a `SectionBody` enum carrying an existing
//! quadraui primitive (`TreeView`, `ListView`, `Form`, `Terminal`,
//! `MessageList`), plain `Text`, an `Empty` welcome state, or a
//! `Custom` escape hatch. Backend rasterisers dispatch to the correct
//! body painter.
//!
//! # Backend contract
//!
//! Two-stage layout:
//! 1. [`MultiSectionView::layout`] resolves chrome bounds (header,
//!    optional aux row, body, optional scrollbar) per section, plus
//!    divider bounds and a flat `hit_regions` list.
//! 2. Backend rasterisers paint headers/aux/scrollbars verbatim and
//!    dispatch each section's body to the correct primitive painter
//!    using the body bounds returned in [`SectionLayout::body_bounds`].
//!
//! This split keeps the chrome layout testable in isolation (no need
//! for tree / list measurers in unit tests) and lets backends paint
//! body content with their native conventions.

use crate::event::Rect;
use crate::primitives::chart::Chart;
use crate::primitives::form::Form;
use crate::primitives::list::ListView;
use crate::primitives::message_list::MessageList;
use crate::primitives::terminal::Terminal;
use crate::primitives::tree::TreeView;
use crate::types::{Icon, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

// ── Public alias-style identifiers ─────────────────────────────────────────

/// Stable identifier for a section. Used by hosts that want to refer to
/// a section by intent rather than by index. Type alias to `String` to
/// match the convention `WidgetId` already uses elsewhere.
pub type SectionId = String;

/// Stable identifier for a header action button.
pub type ActionId = String;

// ── Top-level enums ────────────────────────────────────────────────────────

/// Layout direction for the section stack. Vertical-only rasterisers in
/// v1; horizontal tracked in #294.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Axis {
    #[default]
    Vertical,
    Horizontal,
}

/// Scroll model for a `MultiSectionView`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ScrollMode {
    /// Each section owns its scrollbar; sections are sized by
    /// [`SectionSize`].
    #[default]
    PerSection,
    /// Single panel-level scrollbar; all sections size to their natural
    /// content height ([`SectionSize`] is ignored). All-or-nothing scroll.
    WholePanel,
}

/// Sizing strategy for a single section along the main axis.
///
/// Different sections in the same view can use different strategies —
/// e.g. an SC panel might be `[Fixed(3) /* commit input */, EqualShare,
/// EqualShare, EqualShare]`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum SectionSize {
    /// Exact size in main-axis units (cells / pixels).
    Fixed(u16),
    /// Share of the *original* container, `0.0..=1.0`. Allocated against
    /// the original container size, not the post-fixed remainder. On
    /// total overflow, all percent allocations scale down proportionally.
    Percent(f32),
    /// Proportional weight (CSS `flex` semantics). Sections share the
    /// post-fixed-and-percent remainder by their weight ratios. Weights
    /// `<= 0.0` are treated as zero (the section gets only its `min_size`).
    Weight(f32),
    /// Size to the body's natural content height; clamped between
    /// `min_size` and `max_size` if the section has those set.
    Content,
    /// Equal share of the post-fixed-and-percent-and-weight remainder
    /// among all `EqualShare` sections.
    #[default]
    EqualShare,
}

// ── Sub-structs ────────────────────────────────────────────────────────────

/// Right-aligned action button in a section header (or in a toolbar aux).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderAction {
    pub id: ActionId,
    pub icon: Icon,
    pub tooltip: Option<String>,
    /// Disabled actions render dimmed and are hit-test-inert (clicks
    /// fall through to [`HeaderHit::TitleArea`]).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Header row content for a section.
///
/// Layout: `[chevron] [icon] [title] [badge]                  [actions...]`
/// where chevron is shown only if [`Self::show_chevron`] is `true`, and
/// the actions are right-aligned.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SectionHeader {
    pub icon: Option<Icon>,
    pub title: StyledText,
    /// Right-of-title status indicator (e.g. `"(3)"`, `"● syncing"`).
    pub badge: Option<StyledText>,
    /// Right-aligned action buttons. Hit-tested first so they "punch
    /// through" the title area. Disabled actions fall through.
    #[serde(default)]
    pub actions: Vec<HeaderAction>,
    /// Whether to draw the leading chevron (▾/▸).
    #[serde(default = "default_true")]
    pub show_chevron: bool,
}

/// Empty-state body. Rendered centered (cross-axis) and vertically
/// centered (main-axis) within the body bounds. Covers everything from
/// a plain "No data" up to a VSCode-style welcome view ("Open Folder" /
/// "Clone Repository" buttons).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EmptyBody {
    /// Optional centered icon above the message.
    pub icon: Option<Icon>,
    /// Primary message line.
    pub text: StyledText,
    /// Secondary hint line — smaller / dimmer.
    pub hint: Option<StyledText>,
    /// Optional clickable call-to-action button below the hint.
    pub action: Option<HeaderAction>,
}

/// Single-line text input rendered inline (used for SC commit messages,
/// search-within-section, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineInput {
    pub id: WidgetId,
    pub text: String,
    /// Caret position in chars from the start of `text`.
    #[serde(default)]
    pub caret: usize,
    pub placeholder: Option<String>,
    /// Whether this input currently has keyboard focus.
    #[serde(default)]
    pub has_focus: bool,
}

/// Auxiliary widget rendered between a section's header and its body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SectionAux {
    /// Single-line text input (e.g. SC commit message).
    Input(InlineInput),
    /// Inline action toolbar.
    Toolbar(Vec<HeaderAction>),
    /// Search-within-section input.
    Search(InlineInput),
    /// Escape hatch — host paints in returned aux bounds.
    Custom(WidgetId),
}

/// Body content of a section. Composes existing quadraui primitives;
/// `Custom` is the escape hatch for app-defined widgets the rasteriser
/// can't paint (host paints in returned [`SectionLayout::body_bounds`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SectionBody {
    Tree(TreeView),
    List(ListView),
    Form(Form),
    Terminal(Terminal),
    MessageList(MessageList),
    /// Static styled-line content. One line per `StyledText`.
    Text(Vec<StyledText>),
    /// Chart (sparkline, line, bar).
    Chart(Chart),
    /// Welcome / empty-state view.
    Empty(EmptyBody),
    /// Custom widget — host paints in `body_bounds` after consulting
    /// the layout. Hit-tests for clicks inside Custom return
    /// [`MultiSectionViewHit::Body`] with the section index; the host
    /// is responsible for any sub-element hit-testing.
    Custom(WidgetId),
}

/// One section in a `MultiSectionView`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub id: SectionId,
    pub header: SectionHeader,
    pub body: SectionBody,
    /// Optional widget rendered between header and body (commit input,
    /// search, toolbar).
    #[serde(default)]
    pub aux: Option<SectionAux>,
    pub size: SectionSize,
    #[serde(default)]
    pub collapsed: bool,
    /// Floor on resolved size in main-axis units (after sizing strategy
    /// is applied). `None` means no minimum.
    #[serde(default)]
    pub min_size: Option<u16>,
    /// Ceiling on resolved size in main-axis units. `None` means no
    /// maximum.
    #[serde(default)]
    pub max_size: Option<u16>,
}

// ── Top-level primitive ────────────────────────────────────────────────────

/// Declarative description of a `MultiSectionView` widget.
///
/// # Examples
///
/// ```
/// use quadraui::{
///     EmptyBody, MsvLayoutMetrics, MultiSectionView, MultiSectionViewHit, Rect, Section,
///     SectionBody, SectionHeader, SectionMeasure, SectionSize, WidgetId,
/// };
///
/// let view = MultiSectionView {
///     id: WidgetId::new("msv:sidebar"),
///     sections: vec![Section {
///         id: "explorer".to_string(),
///         header: SectionHeader {
///             title: quadraui::StyledText::plain("Explorer"),
///             ..Default::default()
///         },
///         body: SectionBody::Empty(EmptyBody::default()),
///         aux: None,
///         size: SectionSize::EqualShare,
///         collapsed: false,
///         min_size: None,
///         max_size: None,
///     }],
///     active_section: None,
///     axis: Default::default(),
///     allow_resize: false,
///     allow_collapse: true,
///     scroll_mode: Default::default(),
///     has_focus: false,
///     panel_scroll: 0.0,
/// };
///
/// let bounds = Rect::new(0.0, 0.0, 40.0, 20.0);
/// let layout = view.layout(bounds, MsvLayoutMetrics::default(), |_| SectionMeasure::default());
///
/// assert_eq!(layout.sections.len(), 1);
/// match layout.hit_test(1.0, 0.0) {
///     MultiSectionViewHit::Header { section, .. } => assert_eq!(section, 0),
///     other => panic!("expected a header hit, got {other:?}"),
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiSectionView {
    pub id: WidgetId,
    pub sections: Vec<Section>,
    /// Index of the section that has keyboard focus (for arrow-key
    /// navigation between sections, etc.). `None` means no section has
    /// focus.
    #[serde(default)]
    pub active_section: Option<usize>,
    pub axis: Axis,
    /// Whether dividers between sections are user-draggable.
    #[serde(default)]
    pub allow_resize: bool,
    /// Whether clicking a header (or pressing Space on a focused
    /// header) toggles its `collapsed` flag.
    #[serde(default = "default_true")]
    pub allow_collapse: bool,
    pub scroll_mode: ScrollMode,
    /// Whether the whole view has keyboard focus.
    #[serde(default)]
    pub has_focus: bool,
    /// Panel-level scroll offset in main-axis units. Only consulted in
    /// [`ScrollMode::WholePanel`].
    #[serde(default)]
    pub panel_scroll: f32,
}

// ── Layout output ──────────────────────────────────────────────────────────

/// Per-section resolved bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SectionLayout {
    /// Index into [`MultiSectionView::sections`] this layout describes.
    pub section_idx: usize,
    /// Header row bounds. Always present.
    pub header_bounds: Rect,
    /// Auxiliary widget bounds, if the section has an aux.
    pub aux_bounds: Option<Rect>,
    /// Body content bounds. Has zero height when the section is
    /// collapsed.
    pub body_bounds: Rect,
    /// Per-section scrollbar gutter bounds (only present in
    /// [`ScrollMode::PerSection`] when the body overflows).
    pub scrollbar_bounds: Option<Rect>,
    /// Per-section scrollbar thumb bounds — sub-rect of
    /// [`Self::scrollbar_bounds`] reflecting the inner body's scroll
    /// state. Computed via [`crate::fit_thumb`] from the body's
    /// `scroll_offset` + row count + viewport rows; rounded to
    /// `metrics.cell_quantum` when set so paint and hit-test agree on
    /// integer cells.
    ///
    /// `Some` only when the body type carries a row-based
    /// `scroll_offset` (`Tree`, `List`). For other overflowing body
    /// types the layout still emits a single combined `Thumb` region
    /// over the whole gutter — see the hit-region emission in
    /// `layout_vertical`.
    pub thumb_bounds: Option<Rect>,
    /// Whether this section is collapsed at layout time.
    pub collapsed: bool,
    /// Resolved size of this section along the main axis (header + aux
    /// + body, in container units).
    pub resolved_size: f32,
}

/// Bounds of a draggable divider between two adjacent sections.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DividerBounds {
    /// Index of the section above (or to the left of) this divider.
    pub above: usize,
    /// Index of the section below (or to the right of) this divider.
    pub below: usize,
    /// Hit / paint bounds. Typically a 1-cell strip in TUI or a few
    /// pixels in GTK.
    pub bounds: Rect,
}

/// Hit kind returned by [`MultiSectionViewLayout::hit_test`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiSectionViewHit {
    /// Click landed on a section header. `kind` describes which sub-zone.
    Header { section: usize, kind: HeaderHit },
    /// Click landed in a section's aux row. `kind` carries the sub-zone.
    Aux { section: usize, kind: AuxHit },
    /// Click landed inside a section's body. The host re-tests against
    /// the body's own primitive (`TreeView::hit_test` / `ListView::hit_test`
    /// / etc.) using the body bounds returned in
    /// [`MultiSectionViewLayout::sections`].
    Body { section: usize },
    /// Click landed on a draggable divider.
    Divider { above: usize, below: usize },
    /// Click landed on a section's scrollbar.
    Scrollbar { section: usize, kind: ScrollbarHit },
    /// Click landed on the panel-level scrollbar (only in
    /// [`ScrollMode::WholePanel`]).
    PanelScrollbar { kind: ScrollbarHit },
    /// Click landed inside the view's bounds but not on any interactive
    /// region.
    Inert,
    /// Click landed outside the view's bounds.
    Outside,
}

/// Sub-zone of a section header that was hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderHit {
    /// Leading chevron — explicit collapse/expand.
    Chevron,
    /// Icon, title, or badge area — host decides intent (focus,
    /// activate, toggle).
    TitleArea,
    /// Right-aligned action button. Disabled actions fall through to
    /// `TitleArea` and never produce this variant.
    Action(ActionId),
}

/// Sub-zone of a section aux that was hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuxHit {
    /// Hit on the input region of an `Input` or `Search` aux.
    Input,
    /// Hit on a toolbar action button.
    Action(ActionId),
    /// Hit on the body of a custom aux.
    Custom,
}

/// Sub-zone of a scrollbar that was hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScrollbarHit {
    /// Hit on the thumb (start a drag).
    Thumb,
    /// Hit on the track above/before the thumb (page up).
    TrackBefore,
    /// Hit on the track below/after the thumb (page down).
    TrackAfter,
}

/// Fully-resolved layout for a `MultiSectionView`.
#[derive(Debug, Clone, PartialEq)]
pub struct MultiSectionViewLayout {
    pub bounds: Rect,
    pub axis: Axis,
    pub scroll_mode: ScrollMode,
    pub sections: Vec<SectionLayout>,
    pub dividers: Vec<DividerBounds>,
    /// Panel-level scrollbar bounds (only present in
    /// [`ScrollMode::WholePanel`] when content overflows).
    pub panel_scrollbar: Option<Rect>,
    /// Panel-level scrollbar *thumb* bounds — sub-rect of
    /// [`Self::panel_scrollbar`], computed via [`crate::fit_thumb`] the
    /// same way [`SectionLayout::thumb_bounds`] is. Published here so
    /// callers that need the thumb's exact travel distance for drag
    /// (e.g. [`crate::compose::SidebarSystem`]) read it straight off the
    /// layout instead of re-deriving the thumb-sizing formula themselves
    /// (quadraui#820 — that re-derivation used to drift from this one).
    pub panel_scrollbar_thumb: Option<Rect>,
    /// Ordered hit-region list. Iterated front-to-back by
    /// [`Self::hit_test`].
    pub hit_regions: Vec<(Rect, MultiSectionViewHit)>,
}

impl MultiSectionViewLayout {
    /// Test which interactive region (if any) contains point `(x, y)`.
    pub fn hit_test(&self, x: f32, y: f32) -> MultiSectionViewHit {
        if !self.bounds.contains(crate::event::Point { x, y }) {
            return MultiSectionViewHit::Outside;
        }
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        MultiSectionViewHit::Inert
    }
}

// ── Layout impl ────────────────────────────────────────────────────────────

/// Per-section measurement supplied by the host during layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SectionMeasure {
    /// Natural content height of the body (when sized to content).
    /// In main-axis units. Only consulted for `SectionSize::Content` /
    /// `ContentClamped` sections — pass `0.0` otherwise.
    pub content_size: f32,
    /// Aux row size in main-axis units, or `0.0` if the section has no
    /// aux.
    pub aux_size: f32,
}

impl Default for SectionMeasure {
    fn default() -> Self {
        Self {
            content_size: 0.0,
            aux_size: 0.0,
        }
    }
}

/// Width of a per-section scrollbar in main-cross-axis units (cells for
/// TUI, pixels for GTK). The host passes their native value; we just
/// reserve the space.
///
/// Named `MsvLayoutMetrics` (not just `LayoutMetrics`) since #822: the
/// bare name read as though it were defined in the unrelated
/// `primitives::layout_metrics` module (which in fact imports *this*
/// type) — same-crate naming ambiguity, not a compile clash. Matches the
/// name this type was already re-exported under at the crate root. The
/// old name's deprecated `pub type` alias was removed in issue #1109
/// (zero uses in coord-tui's `main` and vimcode's `develop`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MsvLayoutMetrics {
    /// Header row size in main-axis units (e.g. 1 cell, or
    /// `line_height` pixels).
    pub header_size: f32,
    /// Divider stripe size in main-axis units (0 means no divider strip
    /// drawn between sections).
    pub divider_size: f32,
    /// Scrollbar gutter size in cross-axis units. Reserved on the
    /// trailing edge of each section's body when the body overflows.
    pub scrollbar_size: f32,
    /// Snap distributed section sizes to integer multiples of this
    /// value. `0.0` (default) means no snapping. TUI sets `1.0` so
    /// section bounds align to terminal cells — paint (which rounds to
    /// `u16` rows) and hit-test (which uses raw bounds) then agree by
    /// construction. GTK leaves it `0.0` for sub-pixel layout.
    pub cell_quantum: f32,
}

impl Default for MsvLayoutMetrics {
    fn default() -> Self {
        Self {
            header_size: 1.0,
            divider_size: 0.0,
            scrollbar_size: 1.0,
            cell_quantum: 0.0,
        }
    }
}

// Pre-#822 name of `MsvLayoutMetrics`, `LayoutMetrics`, was removed in
// issue #1109 (zero uses in coord-tui's `main` and vimcode's `develop`).

impl MultiSectionView {
    /// Compute the full chrome layout for this view.
    ///
    /// `measure(section_idx) -> SectionMeasure` reports the body's
    /// natural content size (used by `SectionSize::Content` and
    /// `ContentClamped`) and the aux row size (used to subtract from
    /// the body area). The closure is called once per section.
    pub fn layout<F>(
        &self,
        bounds: Rect,
        metrics: MsvLayoutMetrics,
        measure: F,
    ) -> MultiSectionViewLayout
    where
        F: Fn(usize) -> SectionMeasure,
    {
        match self.axis {
            Axis::Vertical => self.layout_vertical(bounds, metrics, measure),
            Axis::Horizontal => {
                // Per #294: horizontal rasterisers ship in a follow-up.
                // For now, return an empty layout so a misconfigured
                // call doesn't panic — backends surface this as an
                // unrendered widget.
                MultiSectionViewLayout {
                    bounds,
                    axis: self.axis,
                    scroll_mode: self.scroll_mode,
                    sections: Vec::new(),
                    dividers: Vec::new(),
                    panel_scrollbar: None,
                    panel_scrollbar_thumb: None,
                    hit_regions: Vec::new(),
                }
            }
        }
    }

    fn layout_vertical<F>(
        &self,
        bounds: Rect,
        metrics: MsvLayoutMetrics,
        measure: F,
    ) -> MultiSectionViewLayout
    where
        F: Fn(usize) -> SectionMeasure,
    {
        let n = self.sections.len();
        if n == 0 {
            return MultiSectionViewLayout {
                bounds,
                axis: Axis::Vertical,
                scroll_mode: self.scroll_mode,
                sections: Vec::new(),
                dividers: Vec::new(),
                panel_scrollbar: None,
                panel_scrollbar_thumb: None,
                hit_regions: Vec::new(),
            };
        }

        // Pre-compute per-section measures in one pass; reused below.
        let measures: Vec<SectionMeasure> = (0..n).map(&measure).collect();

        // Container main-axis size after subtracting divider stripes
        // between non-collapsed sections. Dividers always sit between
        // sections (not above the first / below the last).
        let divider_count = if metrics.divider_size > 0.0 && n > 1 {
            (n - 1) as f32
        } else {
            0.0
        };
        let dividers_total = metrics.divider_size * divider_count;
        let usable_main = (bounds.height - dividers_total).max(0.0);

        // Per-section "must-have" main-axis cost: header + (aux if any).
        // This is paid even when collapsed for the header.
        let mut chrome_cost: Vec<f32> = vec![0.0; n];
        for i in 0..n {
            let aux = if self.sections[i].aux.is_some() {
                measures[i].aux_size
            } else {
                0.0
            };
            // Collapsed sections only show the header row.
            let chrome = if self.sections[i].collapsed {
                metrics.header_size
            } else {
                metrics.header_size + aux
            };
            chrome_cost[i] = chrome;
        }

        // ── Resolve per-section main-axis sizes ────────────────────
        let mut resolved = match self.scroll_mode {
            ScrollMode::WholePanel => {
                // Every section sized to chrome + content height.
                let mut sizes = Vec::with_capacity(n);
                for i in 0..n {
                    let body = if self.sections[i].collapsed {
                        0.0
                    } else {
                        measures[i].content_size
                    };
                    let total = chrome_cost[i] + body;
                    sizes.push(apply_min_max(&self.sections[i], total));
                }
                sizes
            }
            ScrollMode::PerSection => resolve_per_section_sizes(
                &self.sections,
                &measures,
                &chrome_cost,
                bounds.height,
                usable_main,
            ),
        };

        // Snap resolved sizes to integer cell multiples when the host
        // requests it (TUI). Without this, fractional section sizes
        // (e.g. 5.5 cells) cause paint to round to integer rows while
        // hit_test uses the raw fractional bounds — paint and click
        // disagree about which row is in which section. Distributes the
        // remainder one cell at a time so the sum still equals the
        // (integer) usable_main exactly.
        if metrics.cell_quantum > 0.0 && n > 0 {
            let q = metrics.cell_quantum;
            let target_total: f32 = resolved.iter().sum();
            let target_cells = (target_total / q).round() as i32;
            let mut snapped: Vec<i32> = resolved.iter().map(|s| (s / q).floor() as i32).collect();
            let mut remainder = target_cells - snapped.iter().sum::<i32>();
            // Order the indices by the size of the dropped fractional
            // part so the largest fractional shares get the spare
            // cells first — matches how layout managers usually break
            // ties on integer distribution.
            let mut order: Vec<(usize, f32)> = resolved
                .iter()
                .enumerate()
                .map(|(i, s)| (i, (s / q) - (s / q).floor()))
                .collect();
            order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let mut cursor = 0;
            while remainder > 0 && !order.is_empty() {
                let (idx, _) = order[cursor % order.len()];
                snapped[idx] += 1;
                remainder -= 1;
                cursor += 1;
            }
            // If we overshot (remainder negative), shave from the
            // smallest-fractional sections.
            if remainder < 0 {
                let mut order_rev = order;
                order_rev.reverse();
                let mut c = 0;
                while remainder < 0 && !order_rev.is_empty() {
                    let (idx, _) = order_rev[c % order_rev.len()];
                    if snapped[idx] > 1 {
                        snapped[idx] -= 1;
                        remainder += 1;
                    }
                    c += 1;
                }
            }
            resolved = snapped.into_iter().map(|s| (s as f32) * q).collect();
        }

        // ── Walk and emit per-section layouts + dividers ───────────
        let mut sections_out = Vec::with_capacity(n);
        let mut dividers = Vec::new();
        let mut hit_regions: Vec<(Rect, MultiSectionViewHit)> = Vec::new();

        let panel_sb_w = match self.scroll_mode {
            ScrollMode::WholePanel => {
                let total: f32 = resolved.iter().sum::<f32>() + dividers_total;
                if total > bounds.height {
                    metrics.scrollbar_size
                } else {
                    0.0
                }
            }
            ScrollMode::PerSection => 0.0,
        };

        // For WholePanel mode, sections stack at content size and the
        // viewport scrolls the whole panel. `panel_scroll` shifts every
        // section's y upward; the layout clamps the value internally to
        // `[0, total - viewport]` so sections never scroll past the
        // bottom even if the host stores an over-shot value (host's
        // stored scroll may exceed the clamp; layout only ever applies
        // the clamped value).
        let scroll_offset = match self.scroll_mode {
            ScrollMode::WholePanel => {
                let total: f32 = resolved.iter().sum::<f32>() + dividers_total;
                let max_scroll = (total - bounds.height).max(0.0);
                self.panel_scroll.clamp(0.0, max_scroll)
            }
            ScrollMode::PerSection => 0.0,
        };
        let mut y = bounds.y - scroll_offset;
        for i in 0..n {
            // For PerSection, each section is bounded by its resolved size.
            // For WholePanel, sections stack at content size and the panel
            // itself scrolls (content past bounds.height is clipped by the
            // backend; we still emit hit_regions / sections normally).
            let s_main = resolved[i];
            let s_top = y;

            let header_bounds = Rect::new(
                bounds.x,
                s_top,
                (bounds.width - panel_sb_w).max(0.0),
                metrics.header_size,
            );

            let mut content_top = s_top + metrics.header_size;

            let aux_bounds = if let Some(aux) = &self.sections[i].aux {
                if self.sections[i].collapsed {
                    None
                } else {
                    let aux_h = measures[i].aux_size;
                    let r = Rect::new(bounds.x, content_top, bounds.width, aux_h);
                    content_top += aux_h;
                    let _ = aux; // silences unused-binding warnings on cfg-narrow builds
                    Some(r)
                }
            } else {
                None
            };

            let collapsed = self.sections[i].collapsed;

            // Body and scrollbar split the remainder of this section's
            // main-axis budget.
            let remaining_main = (s_top + s_main - content_top).max(0.0);

            // Decide if a per-section scrollbar is reserved. PerSection
            // mode reserves one if body content overflows the body area;
            // WholePanel never reserves per-section scrollbars.
            let body_main = if collapsed { 0.0 } else { remaining_main };
            let needs_scrollbar = matches!(self.scroll_mode, ScrollMode::PerSection)
                && !collapsed
                && body_overflows(&self.sections[i], measures[i].content_size, body_main);
            let scrollbar_w = if needs_scrollbar {
                metrics.scrollbar_size
            } else {
                0.0
            };
            let body_w = (bounds.width - scrollbar_w - panel_sb_w).max(0.0);

            let body_bounds = Rect::new(bounds.x, content_top, body_w, body_main);
            let scrollbar_bounds = if needs_scrollbar {
                Some(Rect::new(
                    bounds.x + body_w,
                    content_top,
                    scrollbar_w,
                    body_main,
                ))
            } else {
                None
            };

            // Per-section thumb bounds — computed from the body's
            // scroll state when introspectable (`Tree`, `List`). For
            // overflowing bodies without row-based scroll the thumb
            // bounds stay `None`; the hit-region emission below falls
            // back to a single combined-Thumb region covering the
            // whole gutter (preserves the pre-#9 behaviour for those
            // body types).
            let thumb_bounds = match (scrollbar_bounds, body_scroll_state(&self.sections[i].body)) {
                (Some(sb), Some((scroll_rows, row_count))) if row_count > 0 => {
                    Some(compute_thumb_bounds(
                        sb,
                        scroll_rows,
                        row_count,
                        measures[i].content_size,
                        metrics.cell_quantum,
                    ))
                }
                _ => None,
            };

            sections_out.push(SectionLayout {
                section_idx: i,
                header_bounds,
                aux_bounds,
                body_bounds,
                scrollbar_bounds,
                thumb_bounds,
                collapsed,
                resolved_size: s_main,
            });

            // Header hit regions: actions first (right-to-left so the
            // last-declared action gets the rightmost slot). Then
            // chevron at the leading edge if shown. Title area covers
            // whatever remains.
            push_header_hits(&self.sections[i].header, header_bounds, i, &mut hit_regions);

            // Aux hit region: a single Input / Search / Custom rectangle,
            // or per-action regions for Toolbar.
            if let (Some(aux), Some(ar)) = (&self.sections[i].aux, aux_bounds) {
                if !collapsed {
                    push_aux_hits(aux, ar, i, &mut hit_regions);
                }
            }

            // Body hit region: one rectangle that the host re-tests
            // against the inner body's own hit_test.
            if !collapsed && body_bounds.height > 0.0 && body_bounds.width > 0.0 {
                hit_regions.push((body_bounds, MultiSectionViewHit::Body { section: i }));
            }

            // Scrollbar hit regions. When the body's scroll state is
            // introspectable (Tree, List), split the gutter into
            // TrackBefore / Thumb / TrackAfter so consumers can route
            // page-up clicks (track above thumb) and page-down (track
            // below thumb) without re-deriving thumb geometry. For
            // overflowing body types without row-based scroll, fall
            // back to a single combined-Thumb region — same as the
            // pre-#9 behaviour. Hit-region order matters: Thumb
            // first (front-to-back hit_test), then the two tracks.
            if let Some(sb) = scrollbar_bounds {
                if let Some(thumb) = thumb_bounds {
                    hit_regions.push((
                        thumb,
                        MultiSectionViewHit::Scrollbar {
                            section: i,
                            kind: ScrollbarHit::Thumb,
                        },
                    ));
                    let above_h = (thumb.y - sb.y).max(0.0);
                    if above_h > 0.0 {
                        let above = Rect::new(sb.x, sb.y, sb.width, above_h);
                        hit_regions.push((
                            above,
                            MultiSectionViewHit::Scrollbar {
                                section: i,
                                kind: ScrollbarHit::TrackBefore,
                            },
                        ));
                    }
                    let below_y = thumb.y + thumb.height;
                    let below_h = (sb.y + sb.height - below_y).max(0.0);
                    if below_h > 0.0 {
                        let below = Rect::new(sb.x, below_y, sb.width, below_h);
                        hit_regions.push((
                            below,
                            MultiSectionViewHit::Scrollbar {
                                section: i,
                                kind: ScrollbarHit::TrackAfter,
                            },
                        ));
                    }
                } else {
                    hit_regions.push((
                        sb,
                        MultiSectionViewHit::Scrollbar {
                            section: i,
                            kind: ScrollbarHit::Thumb,
                        },
                    ));
                }
            }

            // Divider after this section (not after the last one).
            y += s_main;
            if metrics.divider_size > 0.0 && i + 1 < n && self.allow_resize {
                let d = Rect::new(bounds.x, y, bounds.width, metrics.divider_size);
                dividers.push(DividerBounds {
                    above: i,
                    below: i + 1,
                    bounds: d,
                });
                hit_regions.push((
                    d,
                    MultiSectionViewHit::Divider {
                        above: i,
                        below: i + 1,
                    },
                ));
                y += metrics.divider_size;
            }
        }

        // Panel-level scrollbar (WholePanel mode only). Thumb geometry
        // goes through the same canonical `fit_thumb` helper the
        // per-section scrollbars use (`compute_thumb_bounds` below) —
        // pre-#820 this branch hand-rolled an equivalent-but-separate
        // formula, and `compose::SidebarSystem` re-derived it a third
        // time for drag setup. One formula now; the thumb rect is
        // published on the layout (`panel_scrollbar_thumb`) so drag
        // setup can read it instead of re-deriving it.
        let (panel_scrollbar, panel_scrollbar_thumb) = match self.scroll_mode {
            ScrollMode::WholePanel => {
                let total_content: f32 = resolved.iter().sum::<f32>() + dividers_total;
                if total_content > bounds.height {
                    let sb_w = metrics.scrollbar_size;
                    let r = Rect::new(
                        bounds.x + bounds.width - sb_w,
                        bounds.y,
                        sb_w,
                        bounds.height,
                    );
                    let min_thumb = panel_thumb_min(&metrics);
                    // Deliberately *not* snapped to `metrics.cell_quantum`
                    // (unlike `compute_thumb_bounds`'s per-section thumb):
                    // a whole-cell `quantize_thumb` pass rounds a
                    // near-full-track thumb *up* to fill the track
                    // exactly, zeroing `travel` and making the panel
                    // scrollbar undraggable under a 1-row overflow — see
                    // `tui_panel_drag_works_in_multi_tree_example_shape`.
                    // The paint side rounds this fractional rect to cells
                    // at render time instead (`tui::multi_section_view::
                    // paint_panel_scrollbar`), same as the per-section
                    // paint path's `.round()` fallback.
                    let (thumb_start, thumb_h) = crate::primitives::scrollbar::fit_thumb(
                        self.panel_scroll,
                        total_content,
                        bounds.height,
                        r.height,
                        min_thumb,
                    );
                    let thumb_y = r.y + thumb_start;
                    if thumb_y > r.y {
                        hit_regions.push((
                            Rect::new(r.x, r.y, r.width, thumb_y - r.y),
                            MultiSectionViewHit::PanelScrollbar {
                                kind: ScrollbarHit::TrackBefore,
                            },
                        ));
                    }
                    let thumb_rect = Rect::new(r.x, thumb_y, r.width, thumb_h);
                    hit_regions.push((
                        thumb_rect,
                        MultiSectionViewHit::PanelScrollbar {
                            kind: ScrollbarHit::Thumb,
                        },
                    ));
                    let thumb_bottom = thumb_y + thumb_h;
                    let track_bottom = r.y + r.height;
                    if thumb_bottom < track_bottom {
                        hit_regions.push((
                            Rect::new(r.x, thumb_bottom, r.width, track_bottom - thumb_bottom),
                            MultiSectionViewHit::PanelScrollbar {
                                kind: ScrollbarHit::TrackAfter,
                            },
                        ));
                    }
                    (Some(r), Some(thumb_rect))
                } else {
                    (None, None)
                }
            }
            ScrollMode::PerSection => (None, None),
        };

        MultiSectionViewLayout {
            bounds,
            axis: Axis::Vertical,
            scroll_mode: self.scroll_mode,
            sections: sections_out,
            dividers,
            panel_scrollbar,
            panel_scrollbar_thumb,
            hit_regions,
        }
    }

    /// Apply a divider drag (Q3 — Fixed-on-drag policy).
    ///
    /// `delta` is the signed main-axis distance the divider moved
    /// (positive = section `above` grew, section `below` shrank).
    /// Both adjacent sections become `SectionSize::Fixed(measured)`,
    /// honouring their `min_size`/`max_size`.
    ///
    /// Returns `true` if either section's size changed; `false` if
    /// `delta` was clamped to zero (already at the boundary).
    pub fn resize_divider(
        &mut self,
        above: usize,
        below: usize,
        current_above: f32,
        current_below: f32,
        delta: f32,
    ) -> bool {
        if above >= self.sections.len() || below >= self.sections.len() {
            return false;
        }
        // Clamp delta against both sections' min/max.
        let (lo_above, hi_above) = size_bounds(&self.sections[above]);
        let (lo_below, hi_below) = size_bounds(&self.sections[below]);
        let max_grow_above = hi_above - current_above;
        let max_shrink_above = current_above - lo_above;
        let max_grow_below = hi_below - current_below;
        let max_shrink_below = current_below - lo_below;
        let mut d = delta;
        if d > 0.0 {
            d = d.min(max_grow_above).min(max_shrink_below);
        } else if d < 0.0 {
            d = (-d).min(max_shrink_above).min(max_grow_below).neg();
        }
        if d == 0.0 {
            return false;
        }
        let new_above = (current_above + d).round() as u16;
        let new_below = (current_below - d).round() as u16;
        self.sections[above].size = SectionSize::Fixed(new_above);
        self.sections[below].size = SectionSize::Fixed(new_below);
        true
    }
}

// ── Internal helpers ───────────────────────────────────────────────────────

trait FloatNeg {
    fn neg(self) -> Self;
}
impl FloatNeg for f32 {
    fn neg(self) -> Self {
        -self
    }
}

fn apply_min_max(s: &Section, raw: f32) -> f32 {
    let mut v = raw;
    if let Some(min) = s.min_size {
        v = v.max(min as f32);
    }
    if let Some(max) = s.max_size {
        v = v.min(max as f32);
    }
    v
}

fn size_bounds(s: &Section) -> (f32, f32) {
    let lo = s.min_size.map(|m| m as f32).unwrap_or(0.0);
    let hi = s.max_size.map(|m| m as f32).unwrap_or(f32::INFINITY);
    (lo, hi)
}

/// Minimum thumb height for the panel-level scrollbar, in the same
/// units the rest of the layout uses (cells for TUI, pixels for GTK).
///
/// `cell_quantum > 0.0` flags TUI: the painter clamps thumb height to
/// 1 cell (`.max(1)` in `paint_panel_scrollbar`), so we mirror that
/// here. GTK keeps the historical 8-pixel floor (clamped up to
/// `scrollbar_size` when the gutter is wider).
///
/// Exposed `pub(crate)` so [`crate::compose::SidebarSystem`]'s drag
/// init can compute the same `thumb_h` the layout did, keeping
/// drag-`travel` consistent with the painted thumb's track distance.
/// Pre-#241 the compose helper used a different floor (`line_height`),
/// so in TUI the painted thumb moved ~8× faster than the cursor and
/// the user-facing symptom was "thumb drag does nothing visible".
pub(crate) fn panel_thumb_min(metrics: &MsvLayoutMetrics) -> f32 {
    if metrics.cell_quantum > 0.0 {
        metrics.cell_quantum
    } else {
        metrics.scrollbar_size.max(8.0)
    }
}

fn body_overflows(s: &Section, content: f32, body_main: f32) -> bool {
    // Empty/Custom never claim overflow — host paints whatever it wants.
    match &s.body {
        SectionBody::Empty(_) | SectionBody::Custom(_) => false,
        _ => content > body_main + 0.5, // tolerate sub-cell rounding
    }
}

/// Inner body's scroll state when introspectable. Returns
/// `(scroll_offset_rows, row_count)` for body types that carry a
/// row-based `scroll_offset` field. `None` for bodies without
/// row-based scroll (`Form`, `Terminal`, `MessageList`, `Text`,
/// `Empty`, `Custom`); the layout falls back to a single
/// combined-Thumb hit region for those.
///
/// Per #9: the primitive introspects the body enum directly rather
/// than threading a host-supplied scroll measure through every
/// `MultiSectionView::layout` call. The scroll state already lives
/// on the inner primitive — there's no second source of truth.
fn body_scroll_state(body: &SectionBody) -> Option<(usize, usize)> {
    match body {
        SectionBody::Tree(t) => Some((t.scroll_offset, t.rows.len())),
        SectionBody::List(l) => {
            let title_row = if l.title.is_some() { 1 } else { 0 };
            Some((l.scroll_offset, l.items.len() + title_row))
        }
        _ => None,
    }
}

/// Snap a [`crate::fit_thumb`] result to whole `cell_quantum` units so
/// paint (which rounds to integer cells in TUI) and hit-test (which
/// otherwise sees raw fractional bounds) agree exactly. A no-op when
/// `cell_quantum <= 0.0` (GTK's fractional-pixel bounds pass through
/// unchanged).
///
/// Used by [`compute_thumb_bounds`] (per-section scrollbars) only.
/// *Not* used for the panel-level scrollbar — see the "Deliberately
/// not snapped" note at that branch's `fit_thumb` call in
/// `MultiSectionView::layout_vertical` for why quantising a
/// near-full-track panel thumb would zero out drag `travel`. Panel
/// paint rounds its fractional rect to cells at render time instead
/// (`tui::multi_section_view::paint_panel_scrollbar`).
pub(crate) fn quantize_thumb(
    thumb_start: f32,
    thumb_len: f32,
    track_len: f32,
    cell_quantum: f32,
) -> (f32, f32) {
    if cell_quantum <= 0.0 {
        return (thumb_start, thumb_len);
    }
    let q = cell_quantum;
    let start_q = (thumb_start / q).floor() as i32;
    let end_q = ((thumb_start + thumb_len) / q).ceil() as i32;
    let max_end_q = (track_len / q).round() as i32;
    let end_q = end_q.min(max_end_q);
    let len_q = (end_q - start_q).max(1);
    let start_q = start_q.min(max_end_q - len_q).max(0);
    (start_q as f32 * q, len_q as f32 * q)
}

/// Compute thumb bounds for a per-section scrollbar.
///
/// Shares [`crate::fit_thumb`] with the panel-level scrollbar (see the
/// `fit_thumb` call in the `WholePanel` branch of
/// `MultiSectionView::layout_vertical`) so both kinds of scrollbar size
/// and position their thumb identically. Unlike the panel-level branch,
/// this additionally snaps to `cell_quantum` via [`quantize_thumb`] when
/// set, so paint and hit-test agree on integer cells (TUI) — per-section
/// thumbs don't have the panel scrollbar's near-full-track drag-`travel`
/// concern (see `quantize_thumb`'s doc), so there's no reason not to.
/// Falls through to fractional bounds when `cell_quantum == 0.0` (GTK).
fn compute_thumb_bounds(
    gutter: Rect,
    scroll_rows: usize,
    row_count: usize,
    content_size: f32,
    cell_quantum: f32,
) -> Rect {
    if row_count == 0 || gutter.height <= 0.0 || content_size <= 0.0 {
        return gutter;
    }
    // Convert row-indexed scroll into the same main-axis units as
    // content_size: row_main_unit = content_size / row_count.
    let row_main_unit = content_size / row_count as f32;
    let scroll_main = scroll_rows as f32 * row_main_unit;
    let visible = gutter.height; // viewport main-axis size = gutter height
    let track_len = gutter.height;
    let min_thumb_len = if cell_quantum > 0.0 {
        cell_quantum
    } else {
        1.0
    };
    let (thumb_start, thumb_len) = crate::primitives::scrollbar::fit_thumb(
        scroll_main,
        content_size,
        visible,
        track_len,
        min_thumb_len,
    );
    let (thumb_start, thumb_len) = quantize_thumb(thumb_start, thumb_len, track_len, cell_quantum);
    Rect::new(gutter.x, gutter.y + thumb_start, gutter.width, thumb_len)
}

/// Three-pass main-axis size resolution for `ScrollMode::PerSection`.
///
/// Pass 1 (Fixed): allocate `SectionSize::Fixed`, `Content` /
/// `ContentClamped`, and collapsed-section header heights.
/// Pass 2 (Percent): allocate `SectionSize::Percent` against the
/// *original* container size; if total overflows the remainder, scale
/// proportionally.
/// Pass 3 (Flex): distribute leftover space across `Weight`,
/// `EqualShare`. `Content` already consumed its content size in pass 1.
fn resolve_per_section_sizes(
    sections: &[Section],
    measures: &[SectionMeasure],
    chrome_cost: &[f32],
    container_main: f32,
    usable_main: f32,
) -> Vec<f32> {
    let n = sections.len();
    let mut out = vec![0.0_f32; n];

    // Pass 1: Fixed + Content (collapsed sections always reduce to chrome
    // cost only).
    let mut consumed = 0.0_f32;
    let mut pass1_done = vec![false; n];
    for i in 0..n {
        if sections[i].collapsed {
            // Collapsed: only the header (chrome_cost includes only
            // header for collapsed).
            out[i] = apply_min_max(&sections[i], chrome_cost[i]);
            consumed += out[i];
            pass1_done[i] = true;
            continue;
        }
        match sections[i].size {
            SectionSize::Fixed(rows) => {
                let v = apply_min_max(&sections[i], rows as f32);
                out[i] = v;
                consumed += v;
                pass1_done[i] = true;
            }
            SectionSize::Content => {
                let raw = chrome_cost[i] + measures[i].content_size;
                let v = apply_min_max(&sections[i], raw);
                out[i] = v;
                consumed += v;
                pass1_done[i] = true;
            }
            _ => {}
        }
    }

    // Pass 2: Percent (allocated against container_main).
    let mut percent_indices: Vec<usize> = Vec::new();
    let mut percent_raw: Vec<f32> = Vec::new();
    for i in 0..n {
        if pass1_done[i] {
            continue;
        }
        if let SectionSize::Percent(p) = sections[i].size {
            percent_indices.push(i);
            percent_raw.push(p.clamp(0.0, 1.0) * container_main);
        }
    }
    let percent_total: f32 = percent_raw.iter().sum();
    let percent_budget = (usable_main - consumed).max(0.0);
    let percent_scale = if percent_total > percent_budget && percent_total > 0.0 {
        percent_budget / percent_total
    } else {
        1.0
    };
    for (k, &i) in percent_indices.iter().enumerate() {
        let raw = percent_raw[k] * percent_scale;
        let v = apply_min_max(&sections[i], raw.max(chrome_cost[i]));
        out[i] = v;
        consumed += v;
        pass1_done[i] = true;
    }

    // Pass 3: Weight + EqualShare share remainder.
    let remainder = (usable_main - consumed).max(0.0);
    let mut flex_indices: Vec<usize> = Vec::new();
    let mut weights: Vec<f32> = Vec::new();
    for i in 0..n {
        if pass1_done[i] {
            continue;
        }
        match sections[i].size {
            SectionSize::Weight(w) => {
                flex_indices.push(i);
                weights.push(w.max(0.0));
            }
            SectionSize::EqualShare => {
                flex_indices.push(i);
                weights.push(1.0);
            }
            // Fixed/Content/Percent already handled above.
            _ => {}
        }
    }
    let weight_sum: f32 = weights.iter().sum();
    if !flex_indices.is_empty() && weight_sum > 0.0 {
        for (k, &i) in flex_indices.iter().enumerate() {
            let raw = remainder * (weights[k] / weight_sum);
            let v = apply_min_max(&sections[i], raw.max(chrome_cost[i]));
            out[i] = v;
        }
    } else {
        // No flex sections — leftover space is unused. Apps that want
        // to absorb it should add an `EqualShare` section.
        for &i in &flex_indices {
            out[i] = apply_min_max(&sections[i], chrome_cost[i]);
        }
    }

    out
}

fn push_header_hits(
    header: &SectionHeader,
    bounds: Rect,
    section: usize,
    out: &mut Vec<(Rect, MultiSectionViewHit)>,
) {
    // Estimated chevron width (1 unit). The exact glyph width is
    // backend-dependent; this is a hit-test approximation that matches
    // typical 1-cell or icon-width footprints.
    let chevron_w = if header.show_chevron { 2.0 } else { 0.0 };

    // Action buttons: right-to-left, 2 units each (icon + spacing).
    // Disabled actions don't reserve a hit region — clicks fall through
    // to TitleArea.
    let action_w = 2.0;
    let mut right_cursor = bounds.x + bounds.width;
    for action in header.actions.iter().rev() {
        if !action.enabled {
            continue;
        }
        let r = Rect::new(right_cursor - action_w, bounds.y, action_w, bounds.height);
        out.push((
            r,
            MultiSectionViewHit::Header {
                section,
                kind: HeaderHit::Action(action.id.clone()),
            },
        ));
        right_cursor -= action_w;
    }

    // Chevron region (leading edge).
    if header.show_chevron {
        let r = Rect::new(bounds.x, bounds.y, chevron_w, bounds.height);
        out.push((
            r,
            MultiSectionViewHit::Header {
                section,
                kind: HeaderHit::Chevron,
            },
        ));
    }

    // TitleArea covers the strip between chevron and the leftmost
    // enabled action button.
    let title_x = bounds.x + chevron_w;
    let title_w = (right_cursor - title_x).max(0.0);
    if title_w > 0.0 {
        let r = Rect::new(title_x, bounds.y, title_w, bounds.height);
        out.push((
            r,
            MultiSectionViewHit::Header {
                section,
                kind: HeaderHit::TitleArea,
            },
        ));
    }
}

fn push_aux_hits(
    aux: &SectionAux,
    bounds: Rect,
    section: usize,
    out: &mut Vec<(Rect, MultiSectionViewHit)>,
) {
    match aux {
        SectionAux::Input(_) | SectionAux::Search(_) => {
            out.push((
                bounds,
                MultiSectionViewHit::Aux {
                    section,
                    kind: AuxHit::Input,
                },
            ));
        }
        SectionAux::Toolbar(actions) => {
            let action_w = 2.0;
            let mut x = bounds.x;
            for a in actions {
                if !a.enabled {
                    x += action_w;
                    continue;
                }
                let r = Rect::new(x, bounds.y, action_w, bounds.height);
                out.push((
                    r,
                    MultiSectionViewHit::Aux {
                        section,
                        kind: AuxHit::Action(a.id.clone()),
                    },
                ));
                x += action_w;
            }
        }
        SectionAux::Custom(_) => {
            out.push((
                bounds,
                MultiSectionViewHit::Aux {
                    section,
                    kind: AuxHit::Custom,
                },
            ));
        }
    }
}

// ── PaintSurface chrome paint (#1074, PaintSurface Phase 4) ──────────────

/// Shared chrome-paint helpers for the `MultiSectionView` rasterisers
/// (issue #1074, `PaintSurface` Phase 4, primitive 1/8).
///
/// Only the **chrome** — header row, aux row, per-section and panel-level
/// scrollbars, divider strip, `Text`/`Empty` bodies — moves here. Each
/// pixel backend's own `src/{gtk,macos,win}/multi_section_view.rs` keeps
/// its `draw_multi_section_view` orchestrator and its `paint_body`
/// dispatcher: `SectionBody::Tree`/`List`/`MessageList` still call each
/// backend's own native rasteriser (`gtk::draw_tree`, `win::list::draw_list`,
/// …), which take a raw `(&Context, &pango::Layout)` / `(&ID2D1RenderTarget,
/// &DWrite)` pair, not `&mut dyn PaintSurface` — those primitives haven't
/// been ported onto this trait yet (see `crate::paint_surface`'s own doc
/// for the full ported/unported split). Moving only what's already
/// expressible through the ~15 verbs on `PaintSurface` mirrors exactly how
/// `crate::primitives::form::native_surface_paint` left `FieldKind::Toolbar`
/// (no rounded-rect/hover chrome available) to each backend's own
/// `draw_form` wrapper.
///
/// # Fixed drift (#1074)
///
/// Pre-port, only the Win-GUI and macOS `paint_header`s clipped the title
/// paint to the region between the chevron and the first reserved action
/// slot; `gtk::multi_section_view::paint_header` painted the title
/// unclipped (its own comment — "Pango clips automatically when we don't
/// set width" — was simply wrong: Pango only wraps/ellipsises when a
/// width *is* set on the layout). A title wider than its title/badge
/// region bled ink past the header's own trailing margin on GTK. This
/// shared [`paint_header`] always brackets the title draw in
/// [`PaintSurface::surface_push_clip`]/[`PaintSurface::surface_pop_clip`],
/// unifying on the already-majority-correct behaviour. See
/// `gtk::multi_section_view::tests::gtk_header_clips_long_title_before_right_margin`
/// for the regression test (observed RED against the pre-port
/// unclipped GTK implementation).
///
/// A second drift, not called out in #1074's text but found while
/// porting: macOS's `paint_panel_scrollbar` (pre-port) recomputed the
/// panel thumb's position/size itself from `(scroll, total_content)`
/// instead of consuming the layout's own `panel_scrollbar_thumb` —
/// exactly the quadraui#820 bug already fixed for GTK/Win. Routing macOS
/// through this shared [`paint_panel_scrollbar`] (which only ever
/// consumes a pre-computed `thumb_bounds`) closes that gap too.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{EmptyBody, SectionAux, SectionHeader};
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;
    use crate::types::{Color, StyledText};

    fn plain_text(t: &StyledText) -> String {
        t.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// Paint a section header row: background fill, optional leading
    /// chevron, right-aligned action glyphs (right-to-left), then the
    /// title — clipped to the region between the chevron and the first
    /// reserved action slot so it can never bleed past it — and an
    /// optional badge after the title. See this module's doc for the
    /// drift this clip closes.
    pub(crate) fn paint_header(
        surface: &mut dyn PaintSurface,
        bounds: Rect,
        header: &SectionHeader,
        collapsed: bool,
        theme: &Theme,
    ) {
        surface.surface_fill_rect(bounds, theme.header_bg);

        let mut left_x = bounds.x + 4.0;
        let row_text_y = |th: f32| (bounds.y + (bounds.height - th) * 0.4).round();

        if header.show_chevron {
            let chevron = if collapsed { "\u{25B8}" } else { "\u{25BE}" };
            let (cw, ch) = surface.surface_measure_text(chevron);
            surface.surface_draw_text_run(
                Rect::new(left_x, row_text_y(ch), cw, ch),
                chevron,
                theme.header_fg,
            );
            left_x += cw + 4.0;
        }

        // Right-aligned actions, right-to-left.
        let mut right_x = bounds.x + bounds.width - 4.0;
        for action in header.actions.iter().rev() {
            let glyph = action.icon.fallback.as_str();
            let (gw, gh) = surface.surface_measure_text(glyph);
            right_x -= gw;
            if right_x < left_x {
                break;
            }
            let action_fg = if action.enabled {
                theme.header_fg
            } else {
                theme.muted_fg
            };
            surface.surface_draw_text_run(
                Rect::new(right_x, row_text_y(gh), gw, gh),
                glyph,
                action_fg,
            );
            right_x -= 8.0;
        }

        // Title text, clipped to `[left_x, right_x)` — see module doc.
        let title_text = plain_text(&header.title);
        if !title_text.is_empty() {
            let (tw, th) = surface.surface_measure_text(&title_text);
            let max_w = (right_x - left_x).max(0.0);
            if max_w > 0.0 {
                surface.surface_push_clip(Rect::new(left_x, bounds.y, max_w, bounds.height));
                surface.surface_draw_text_run(
                    Rect::new(left_x, row_text_y(th), tw, th),
                    &title_text,
                    theme.header_fg,
                );
                surface.surface_pop_clip();
                let after_title_x = left_x + tw.min(max_w);

                if let Some(badge) = &header.badge {
                    let badge_text = plain_text(badge);
                    if !badge_text.is_empty() {
                        let badge_x = after_title_x + 6.0;
                        if badge_x < right_x {
                            let (bw, bth) = surface.surface_measure_text(&badge_text);
                            surface.surface_draw_text_run(
                                Rect::new(badge_x, bounds.y + (bounds.height - bth) / 2.0, bw, bth),
                                &badge_text,
                                theme.muted_fg,
                            );
                        }
                    }
                }
            }
        }
    }

    /// Paint a section's aux row (`Input`/`Search`/`Toolbar`/`Custom`).
    ///
    /// `caret_visible` lets a caller with its own blink-timer state
    /// (macOS, #188) gate the caret paint; a caller without one (GTK,
    /// Win) passes `true` unconditionally, reproducing their pre-port
    /// always-on-while-focused behaviour exactly.
    pub(crate) fn paint_aux(
        surface: &mut dyn PaintSurface,
        bounds: Rect,
        aux: &SectionAux,
        theme: &Theme,
        caret_visible: bool,
    ) {
        surface.surface_fill_rect(bounds, theme.input_bg);

        match aux {
            SectionAux::Input(input) | SectionAux::Search(input) => {
                let display: &str = if input.text.is_empty() && !input.has_focus {
                    input.placeholder.as_deref().unwrap_or("")
                } else {
                    input.text.as_str()
                };
                let text_fg = if input.text.is_empty() && !input.has_focus {
                    theme.muted_fg
                } else {
                    theme.foreground
                };
                let (dw, dh) = surface.surface_measure_text(display);
                surface.surface_draw_text_run(
                    Rect::new(
                        bounds.x + 4.0,
                        bounds.y + (bounds.height - dh) / 2.0,
                        dw,
                        dh,
                    ),
                    display,
                    text_fg,
                );

                if input.has_focus && caret_visible {
                    let prefix: String = input.text.chars().take(input.caret).collect();
                    let (cx_off, _) = surface.surface_measure_text(&prefix);
                    let caret_x = bounds.x + 4.0 + cx_off;
                    surface.surface_fill_rect(
                        Rect::new(caret_x, bounds.y + 2.0, 1.0, bounds.height - 4.0),
                        theme.foreground,
                    );
                }
            }
            SectionAux::Toolbar(actions) => {
                let mut tx = bounds.x + 4.0;
                for a in actions {
                    let glyph = a.icon.fallback.as_str();
                    let action_fg = if a.enabled {
                        theme.foreground
                    } else {
                        theme.muted_fg
                    };
                    let (gw, gh) = surface.surface_measure_text(glyph);
                    surface.surface_draw_text_run(
                        Rect::new(tx, bounds.y + (bounds.height - gh) / 2.0, gw, gh),
                        glyph,
                        action_fg,
                    );
                    tx += gw + 8.0;
                }
            }
            SectionAux::Custom(_) => {
                // Host paints; we cleared the bg already.
            }
        }
    }

    /// Paint a `SectionBody::Text` body: background fill, then one row
    /// per `StyledText` line (dropped once rows would overflow `bounds`).
    pub(crate) fn paint_text_lines(
        surface: &mut dyn PaintSurface,
        bounds: Rect,
        lines: &[StyledText],
        theme: &Theme,
        line_height: f32,
    ) {
        surface.surface_fill_rect(bounds, theme.background);
        let mut row_y = bounds.y;
        for line in lines {
            if row_y + line_height > bounds.y + bounds.height {
                break;
            }
            let text = plain_text(line);
            let (tw, th) = surface.surface_measure_text(&text);
            surface.surface_draw_text_run(
                Rect::new(bounds.x + 4.0, row_y + (line_height - th) / 2.0, tw, th),
                &text,
                theme.foreground,
            );
            row_y += line_height;
        }
    }

    /// Paint a `SectionBody::Empty` welcome/empty state: background
    /// fill, then a vertically-centred, individually horizontally-centred
    /// stack of (optional icon, primary text, optional hint, optional
    /// action) blocks.
    pub(crate) fn paint_empty_body(
        surface: &mut dyn PaintSurface,
        bounds: Rect,
        empty: &EmptyBody,
        theme: &Theme,
        line_height: f32,
    ) {
        surface.surface_fill_rect(bounds, theme.background);
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }

        let mut blocks: Vec<(String, Color)> = Vec::new();
        if let Some(icon) = &empty.icon {
            blocks.push((icon.fallback.clone(), theme.foreground));
        }
        let primary = plain_text(&empty.text);
        if !primary.is_empty() {
            blocks.push((primary, theme.foreground));
        }
        if let Some(hint) = &empty.hint {
            let hint_str = plain_text(hint);
            if !hint_str.is_empty() {
                blocks.push((hint_str, theme.muted_fg));
            }
        }
        if let Some(action) = &empty.action {
            let label = action
                .tooltip
                .clone()
                .unwrap_or_else(|| action.icon.fallback.clone());
            blocks.push((format!("[ {label} ]"), theme.accent_fg));
        }
        if blocks.is_empty() {
            return;
        }

        let total_h = blocks.len() as f32 * line_height;
        let mut block_y = bounds.y + (bounds.height - total_h).max(0.0) / 2.0;
        for (text, color) in &blocks {
            let (tw, th) = surface.surface_measure_text(text);
            let block_x = bounds.x + (bounds.width - tw).max(0.0) / 2.0;
            surface.surface_draw_text_run(
                Rect::new(block_x, block_y + (line_height - th) / 2.0, tw, th),
                text,
                *color,
            );
            block_y += line_height;
        }
    }

    /// Paint a per-section scrollbar gutter: a 50%-alpha track with a
    /// 90%-alpha thumb on top, both real alpha blends against whatever
    /// is already painted underneath (matching GTK's/macOS's pre-port
    /// behaviour; Win-GUI's `D2dSurface::surface_fill_rect` has honoured
    /// real alpha since #791/#1072, so routing it through here is not a
    /// behaviour change for Win either — see this module's doc for the
    /// one drift this port does fix).
    ///
    /// `thumb_bounds` comes from the layout when the body's scroll state
    /// is introspectable (`Tree`, `List`); `None` falls back to a
    /// 20%-tall top-anchored placeholder thumb, preserving the pre-#9
    /// visual for other overflowing body types.
    pub(crate) fn paint_scrollbar(
        surface: &mut dyn PaintSurface,
        gutter: Rect,
        thumb_bounds: Option<Rect>,
        theme: &Theme,
    ) {
        surface.surface_fill_rect(gutter, theme.scrollbar_track.with_alpha(0.5));

        let (ty, th) = match thumb_bounds {
            Some(t) => (t.y, t.height.max(1.0)),
            None => (gutter.y, (gutter.height * 0.2).max(20.0).min(gutter.height)),
        };
        surface.surface_fill_rect(
            Rect::new(gutter.x, ty, gutter.width, th),
            theme.scrollbar_thumb.with_alpha(0.9),
        );
    }

    /// Paint the panel-level scrollbar (`ScrollMode::WholePanel`): an
    /// opaque track, then an opaque thumb at `thumb_bounds` — geometry
    /// computed once by
    /// [`crate::primitives::multi_section_view::MultiSectionView::layout`]
    /// (`fit_thumb`) and published as
    /// [`crate::primitives::multi_section_view::MultiSectionViewLayout::panel_scrollbar_thumb`].
    /// Never re-derived here — see this module's doc for the quadraui#820
    /// class of bug that re-deriving it caused on three of the four
    /// backends that have ever painted this scrollbar (GTK, Win, and now
    /// fixed here, macOS).
    pub(crate) fn paint_panel_scrollbar(
        surface: &mut dyn PaintSurface,
        bounds: Rect,
        thumb_bounds: Option<Rect>,
        theme: &Theme,
    ) {
        if bounds.height <= 0.0 {
            return;
        }
        surface.surface_fill_rect(bounds, theme.scrollbar_track);
        let (thumb_y, thumb_h) = match thumb_bounds {
            Some(t) => (t.y, t.height.max(1.0)),
            None => (bounds.y, bounds.height),
        };
        surface.surface_fill_rect(
            Rect::new(bounds.x, thumb_y, bounds.width, thumb_h),
            theme.scrollbar_thumb,
        );
    }

    /// Paint a draggable divider strip between two adjacent sections.
    pub(crate) fn paint_divider(surface: &mut dyn PaintSurface, bounds: Rect, theme: &Theme) {
        surface.surface_fill_rect(bounds, theme.separator);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::multi_section_view::{HeaderAction, InlineInput};
        use crate::types::WidgetId;
        use crate::Image;

        /// Records every draw verb this module's functions call, plus
        /// clip push/pop order — mirrors `primitives::scrollbar`'s
        /// `RecordingSurface` test double, scoped to the verbs these
        /// chrome functions use, so these tests run on any host without
        /// Cairo/Core Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            /// One entry per `surface_draw_text_run` call: `(rect, text,
            /// color, clip_depth)` — `clip_depth` is `clips.len()` at
            /// the moment of the call, so a test can tell whether a
            /// given text draw happened while a clip was active.
            texts: Vec<(Rect, String, Color, usize)>,
            /// Stack of pushed clip rects; popped on `surface_pop_clip`.
            clips: Vec<Rect>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(200.0, 100.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 14.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.texts
                    .push((rect, text.to_string(), color, self.clips.len()));
            }
            fn surface_draw_line(
                &mut self,
                _from: crate::Point,
                _to: crate::Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, rect: Rect) {
                self.clips.push(rect);
            }
            fn surface_pop_clip(&mut self) {
                self.clips.pop();
            }
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn header_with(title: &str, actions: Vec<HeaderAction>) -> SectionHeader {
            SectionHeader {
                icon: None,
                title: StyledText::plain(title),
                badge: None,
                actions,
                show_chevron: false,
            }
        }

        /// Regression for #1074: the title text draw must always happen
        /// while exactly one clip is active (pushed immediately before,
        /// popped immediately after) — the drift this port fixes was
        /// GTK's `paint_header` never pushing that clip at all.
        #[test]
        fn paint_header_clips_the_title_draw() {
            let header = header_with("a very long title indeed", vec![]);
            let mut surface = RecordingSurface::default();
            paint_header(
                &mut surface,
                Rect::new(0.0, 0.0, 120.0, 20.0),
                &header,
                false,
                &Theme::default(),
            );

            let title_draw = surface
                .texts
                .iter()
                .find(|(_, text, ..)| text == "a very long title indeed")
                .expect("title must be drawn");
            assert_eq!(
                title_draw.3, 1,
                "title draw must happen with exactly one clip active, got depth {}",
                title_draw.3
            );
            assert!(
                surface.clips.is_empty(),
                "every pushed clip must be popped by the time paint_header returns"
            );
        }

        /// No title text at all ⇒ no clip is pushed (nothing to bracket).
        #[test]
        fn paint_header_skips_clip_when_title_is_empty() {
            let header = header_with("", vec![]);
            let mut surface = RecordingSurface::default();
            paint_header(
                &mut surface,
                Rect::new(0.0, 0.0, 120.0, 20.0),
                &header,
                false,
                &Theme::default(),
            );
            assert!(surface.clips.is_empty());
            assert!(surface.texts.is_empty());
        }

        /// Regression for quadraui#791, ported to the shared chrome
        /// path: per-section scrollbar track/thumb must carry real
        /// alpha, never a fully-opaque colour pre-blended against a
        /// hardcoded destination.
        #[test]
        fn scrollbar_track_and_thumb_paint_with_real_alpha() {
            let mut surface = RecordingSurface::default();
            paint_scrollbar(
                &mut surface,
                Rect::new(0.0, 0.0, 8.0, 100.0),
                Some(Rect::new(0.0, 10.0, 8.0, 20.0)),
                &Theme::default(),
            );
            assert_eq!(surface.fills.len(), 2);
            for (_, color) in &surface.fills {
                assert!(color.a < 255, "expected real alpha, got a={}", color.a);
            }
        }

        /// The panel-level scrollbar must consume the layout-supplied
        /// `thumb_bounds` verbatim rather than re-deriving thumb geometry
        /// from `(scroll, total)` — the quadraui#820 class of bug this
        /// module's doc calls out as still present, pre-port, on macOS.
        #[test]
        fn panel_scrollbar_uses_supplied_thumb_bounds_not_a_recomputed_one() {
            let mut surface = RecordingSurface::default();
            let thumb = Rect::new(0.0, 37.0, 8.0, 15.0);
            paint_panel_scrollbar(
                &mut surface,
                Rect::new(0.0, 0.0, 8.0, 100.0),
                Some(thumb),
                &Theme::default(),
            );
            let thumb_fill = surface
                .fills
                .iter()
                .find(|(r, _)| {
                    (r.y - thumb.y).abs() < 0.01 && (r.height - thumb.height).abs() < 0.01
                })
                .expect("thumb must paint at the exact supplied thumb_bounds");
            assert_eq!(thumb_fill.0, thumb);
        }

        #[test]
        fn empty_body_with_no_content_paints_only_background() {
            let mut surface = RecordingSurface::default();
            paint_empty_body(
                &mut surface,
                Rect::new(0.0, 0.0, 100.0, 100.0),
                &EmptyBody::default(),
                &Theme::default(),
                16.0,
            );
            assert_eq!(surface.fills.len(), 1, "only the background fill");
            assert!(surface.texts.is_empty());
        }

        #[test]
        fn aux_caret_paints_only_when_focused_and_visible() {
            let aux = SectionAux::Search(InlineInput {
                id: WidgetId::new("q"),
                text: String::new(),
                caret: 0,
                placeholder: None,
                has_focus: true,
            });
            let bounds = Rect::new(0.0, 0.0, 100.0, 16.0);

            let mut visible = RecordingSurface::default();
            paint_aux(&mut visible, bounds, &aux, &Theme::default(), true);
            // Background + (empty display text, still measured/drawn as
            // an empty string) + caret fill.
            assert_eq!(
                visible.fills.len(),
                2,
                "expected background fill + caret fill"
            );

            let mut hidden = RecordingSurface::default();
            paint_aux(&mut hidden, bounds, &aux, &Theme::default(), false);
            assert_eq!(
                hidden.fills.len(),
                1,
                "caret_visible=false must skip the caret fill"
            );
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::WidgetId;

    fn empty_section(id: &str, size: SectionSize) -> Section {
        Section {
            id: id.into(),
            header: SectionHeader {
                title: StyledText::plain(id),
                show_chevron: true,
                ..Default::default()
            },
            body: SectionBody::Empty(EmptyBody {
                text: StyledText::plain("empty"),
                ..Default::default()
            }),
            aux: None,
            size,
            collapsed: false,
            min_size: None,
            max_size: None,
        }
    }

    fn view(sections: Vec<Section>) -> MultiSectionView {
        MultiSectionView {
            id: WidgetId::new("view"),
            sections,
            active_section: None,
            axis: Axis::Vertical,
            allow_resize: false,
            allow_collapse: true,
            scroll_mode: ScrollMode::PerSection,
            has_focus: false,
            panel_scroll: 0.0,
        }
    }

    #[test]
    fn equal_share_two_sections_split_evenly() {
        let v = view(vec![
            empty_section("a", SectionSize::EqualShare),
            empty_section("b", SectionSize::EqualShare),
        ]);
        let layout = v.layout(
            Rect::new(0.0, 0.0, 30.0, 20.0),
            MsvLayoutMetrics::default(),
            |_| SectionMeasure::default(),
        );
        // 20 rows total - 0 dividers = 20 usable; both equal share = 10 each.
        assert_eq!(layout.sections.len(), 2);
        assert!((layout.sections[0].resolved_size - 10.0).abs() < 0.01);
        assert!((layout.sections[1].resolved_size - 10.0).abs() < 0.01);
        assert_eq!(layout.sections[0].header_bounds.y, 0.0);
        assert_eq!(layout.sections[1].header_bounds.y, 10.0);
    }

    #[test]
    fn fixed_then_equal_share_splits_remainder() {
        let v = view(vec![
            empty_section("fixed", SectionSize::Fixed(5)),
            empty_section("a", SectionSize::EqualShare),
            empty_section("b", SectionSize::EqualShare),
        ]);
        let layout = v.layout(
            Rect::new(0.0, 0.0, 30.0, 25.0),
            MsvLayoutMetrics::default(),
            |_| SectionMeasure::default(),
        );
        // 25 - 5 (fixed) = 20 remaining; equal split = 10 each.
        assert!((layout.sections[0].resolved_size - 5.0).abs() < 0.01);
        assert!((layout.sections[1].resolved_size - 10.0).abs() < 0.01);
        assert!((layout.sections[2].resolved_size - 10.0).abs() < 0.01);
    }

    #[test]
    fn percent_allocates_against_original_container() {
        let v = view(vec![
            empty_section("p", SectionSize::Percent(0.4)),
            empty_section("rest", SectionSize::EqualShare),
        ]);
        let layout = v.layout(
            Rect::new(0.0, 0.0, 30.0, 100.0),
            MsvLayoutMetrics::default(),
            |_| SectionMeasure::default(),
        );
        // Percent(0.4) of 100 = 40; rest = 60.
        assert!((layout.sections[0].resolved_size - 40.0).abs() < 0.01);
        assert!((layout.sections[1].resolved_size - 60.0).abs() < 0.01);
    }

    #[test]
    fn weight_distributes_2_to_1() {
        let v = view(vec![
            empty_section("a", SectionSize::Weight(2.0)),
            empty_section("b", SectionSize::Weight(1.0)),
        ]);
        let layout = v.layout(
            Rect::new(0.0, 0.0, 30.0, 30.0),
            MsvLayoutMetrics::default(),
            |_| SectionMeasure::default(),
        );
        // 30 split 2:1 = 20:10.
        assert!((layout.sections[0].resolved_size - 20.0).abs() < 0.01);
        assert!((layout.sections[1].resolved_size - 10.0).abs() < 0.01);
    }

    #[test]
    fn collapsed_section_uses_only_header_height() {
        let mut sections = vec![
            empty_section("a", SectionSize::EqualShare),
            empty_section("b", SectionSize::EqualShare),
        ];
        sections[0].collapsed = true;
        let v = view(sections);
        let layout = v.layout(
            Rect::new(0.0, 0.0, 30.0, 20.0),
            MsvLayoutMetrics::default(),
            |_| SectionMeasure::default(),
        );
        // Collapsed section gets header_size (1.0); other gets remainder (19).
        assert!((layout.sections[0].resolved_size - 1.0).abs() < 0.01);
        assert!((layout.sections[1].resolved_size - 19.0).abs() < 0.01);
        assert!(layout.sections[0].collapsed);
    }

    #[test]
    fn min_size_floor_honoured_for_equal_share() {
        let mut sections = vec![
            empty_section("a", SectionSize::EqualShare),
            empty_section("b", SectionSize::EqualShare),
        ];
        sections[0].min_size = Some(15);
        let v = view(sections);
        let layout = v.layout(
            Rect::new(0.0, 0.0, 30.0, 20.0),
            MsvLayoutMetrics::default(),
            |_| SectionMeasure::default(),
        );
        // Without min: both 10. With min(a)=15: a is at least 15.
        assert!(layout.sections[0].resolved_size >= 15.0 - 0.01);
    }

    #[test]
    fn header_hit_chevron_vs_title_vs_action() {
        let header = SectionHeader {
            title: StyledText::plain("Section"),
            show_chevron: true,
            actions: vec![HeaderAction {
                id: "refresh".into(),
                icon: Icon::new("", "R"),
                tooltip: None,
                enabled: true,
            }],
            ..Default::default()
        };
        let v = view(vec![Section {
            id: "s".into(),
            header,
            body: SectionBody::Empty(EmptyBody::default()),
            aux: None,
            size: SectionSize::Fixed(3),
            collapsed: false,
            min_size: None,
            max_size: None,
        }]);
        let layout = v.layout(
            Rect::new(0.0, 0.0, 30.0, 10.0),
            MsvLayoutMetrics::default(),
            |_| SectionMeasure::default(),
        );
        // Chevron at x=0.
        match layout.hit_test(0.5, 0.5) {
            MultiSectionViewHit::Header {
                kind: HeaderHit::Chevron,
                ..
            } => {}
            other => panic!("expected Chevron, got {:?}", other),
        }
        // Title area in the middle.
        match layout.hit_test(10.0, 0.5) {
            MultiSectionViewHit::Header {
                kind: HeaderHit::TitleArea,
                ..
            } => {}
            other => panic!("expected TitleArea, got {:?}", other),
        }
        // Action at the right edge (within last 2 units, i.e. x in [28,30)).
        match layout.hit_test(29.0, 0.5) {
            MultiSectionViewHit::Header {
                kind: HeaderHit::Action(id),
                ..
            } => {
                assert_eq!(id, "refresh");
            }
            other => panic!("expected Action, got {:?}", other),
        }
    }

    #[test]
    fn disabled_action_falls_through_to_title_area() {
        let header = SectionHeader {
            title: StyledText::plain("Section"),
            show_chevron: false,
            actions: vec![HeaderAction {
                id: "noop".into(),
                icon: Icon::new("", "x"),
                tooltip: None,
                enabled: false,
            }],
            ..Default::default()
        };
        let v = view(vec![Section {
            id: "s".into(),
            header,
            body: SectionBody::Empty(EmptyBody::default()),
            aux: None,
            size: SectionSize::Fixed(3),
            collapsed: false,
            min_size: None,
            max_size: None,
        }]);
        let layout = v.layout(
            Rect::new(0.0, 0.0, 30.0, 10.0),
            MsvLayoutMetrics::default(),
            |_| SectionMeasure::default(),
        );
        // Click in the action zone — but action is disabled, so we expect TitleArea.
        match layout.hit_test(29.0, 0.5) {
            MultiSectionViewHit::Header {
                kind: HeaderHit::TitleArea,
                ..
            } => {}
            other => panic!("expected TitleArea, got {:?}", other),
        }
    }

    #[test]
    fn body_hit_returns_section_index() {
        let v = view(vec![
            empty_section("a", SectionSize::EqualShare),
            empty_section("b", SectionSize::EqualShare),
        ]);
        let layout = v.layout(
            Rect::new(0.0, 0.0, 30.0, 20.0),
            MsvLayoutMetrics::default(),
            |_| SectionMeasure::default(),
        );
        // Section 0 body is at y=1..10 (header at y=0..1).
        match layout.hit_test(15.0, 5.0) {
            MultiSectionViewHit::Body { section } => assert_eq!(section, 0),
            other => panic!("expected Body, got {:?}", other),
        }
        // Section 1 body is at y=11..20 (header at y=10..11).
        match layout.hit_test(15.0, 15.0) {
            MultiSectionViewHit::Body { section } => assert_eq!(section, 1),
            other => panic!("expected Body, got {:?}", other),
        }
    }

    #[test]
    fn outside_bounds_returns_outside() {
        let v = view(vec![empty_section("a", SectionSize::EqualShare)]);
        let layout = v.layout(
            Rect::new(10.0, 10.0, 20.0, 20.0),
            MsvLayoutMetrics::default(),
            |_| SectionMeasure::default(),
        );
        match layout.hit_test(0.0, 0.0) {
            MultiSectionViewHit::Outside => {}
            other => panic!("expected Outside, got {:?}", other),
        }
    }

    #[test]
    fn divider_only_emitted_when_resize_allowed() {
        let mut v = view(vec![
            empty_section("a", SectionSize::EqualShare),
            empty_section("b", SectionSize::EqualShare),
        ]);
        let metrics = MsvLayoutMetrics {
            divider_size: 1.0,
            ..MsvLayoutMetrics::default()
        };
        v.allow_resize = false;
        let layout = v.layout(Rect::new(0.0, 0.0, 30.0, 20.0), metrics, |_| {
            SectionMeasure::default()
        });
        assert!(layout.dividers.is_empty());
        v.allow_resize = true;
        let layout = v.layout(Rect::new(0.0, 0.0, 30.0, 20.0), metrics, |_| {
            SectionMeasure::default()
        });
        assert_eq!(layout.dividers.len(), 1);
        assert_eq!(layout.dividers[0].above, 0);
        assert_eq!(layout.dividers[0].below, 1);
    }

    #[test]
    fn divider_drag_clamped_by_min_max() {
        let mut sections = vec![
            empty_section("a", SectionSize::Fixed(10)),
            empty_section("b", SectionSize::Fixed(10)),
        ];
        sections[0].min_size = Some(5);
        sections[1].min_size = Some(5);
        let mut v = view(sections);
        // Try to grow A by 100; should be clamped to B's min (10 - 5 = 5).
        let changed = v.resize_divider(0, 1, 10.0, 10.0, 100.0);
        assert!(changed);
        match v.sections[0].size {
            SectionSize::Fixed(n) => assert_eq!(n, 15),
            _ => panic!("expected Fixed"),
        }
        match v.sections[1].size {
            SectionSize::Fixed(n) => assert_eq!(n, 5),
            _ => panic!("expected Fixed"),
        }
    }

    #[test]
    fn whole_panel_scroll_mode_no_per_section_scrollbars() {
        let mut sections = vec![
            empty_section("a", SectionSize::Content),
            empty_section("b", SectionSize::Content),
        ];
        // Force content-sized bodies. Empty body says no overflow (per
        // body_overflows), so we use Tree-flavoured behaviour: swap to
        // an Empty body but pretend content is non-zero. body_overflows
        // returns false for Empty; but in WholePanel mode we never
        // emit per-section scrollbars regardless, which is what we're
        // checking here.
        sections[0].body = SectionBody::Text(vec![StyledText::plain("line1")]);
        sections[1].body = SectionBody::Text(vec![StyledText::plain("line2")]);
        let mut v = view(sections);
        v.scroll_mode = ScrollMode::WholePanel;
        let layout = v.layout(
            Rect::new(0.0, 0.0, 30.0, 4.0),
            MsvLayoutMetrics::default(),
            |_| SectionMeasure {
                content_size: 5.0,
                aux_size: 0.0,
            },
        );
        for s in &layout.sections {
            assert!(s.scrollbar_bounds.is_none());
        }
        // Total content (each section header 1 + content 5 = 6, ×2 = 12)
        // exceeds bounds.height (4) → panel scrollbar.
        assert!(layout.panel_scrollbar.is_some());
    }

    #[test]
    fn cell_quantum_snaps_section_bounds_to_integers() {
        // Regression: with a fractional EqualShare distribution
        // (4 sections in 21 cells → 5.25 each), paint snaps via
        // `bounds.y.round()` while hit_test used to consume raw f32
        // bounds. Click at the integer row that paint drew the header
        // on resolved to the previous section's body, breaking
        // every section after the first. With `cell_quantum: 1.0`
        // the layout itself snaps to whole cells so paint and click
        // can never disagree.
        let v = view(vec![
            empty_section("a", SectionSize::EqualShare),
            empty_section("b", SectionSize::EqualShare),
            empty_section("c", SectionSize::EqualShare),
            empty_section("d", SectionSize::EqualShare),
        ]);
        let metrics = MsvLayoutMetrics {
            cell_quantum: 1.0,
            ..MsvLayoutMetrics::default()
        };
        let layout = v.layout(Rect::new(0.0, 0.0, 30.0, 21.0), metrics, |_| {
            SectionMeasure {
                content_size: 3.0,
                aux_size: 0.0,
            }
        });
        for s in &layout.sections {
            let hb = s.header_bounds;
            assert_eq!(
                hb.y,
                hb.y.round(),
                "section {} header y {} not integer-aligned",
                s.section_idx,
                hb.y
            );
            assert_eq!(
                hb.height,
                hb.height.round(),
                "section {} header h {} not integer-aligned",
                s.section_idx,
                hb.height
            );
            let bb = s.body_bounds;
            assert_eq!(
                bb.y,
                bb.y.round(),
                "section {} body y {} not integer-aligned",
                s.section_idx,
                bb.y
            );
            assert_eq!(
                bb.height,
                bb.height.round(),
                "section {} body h {} not integer-aligned",
                s.section_idx,
                bb.height
            );
        }
        // Sum of resolved sizes equals usable_main exactly (21 cells).
        let total: f32 = layout.sections.iter().map(|s| s.resolved_size).sum();
        assert_eq!(total, 21.0);
    }

    #[test]
    fn cell_quantum_paint_and_hit_test_agree_on_every_row() {
        // For each integer row in the panel, the row paint draws a
        // header on (rounding `header_bounds.y`) must equal the row
        // hit_test resolves to that section's header. With
        // `cell_quantum: 1.0` this is true by construction.
        let v = view(vec![
            empty_section("a", SectionSize::EqualShare),
            empty_section("b", SectionSize::EqualShare),
            empty_section("c", SectionSize::EqualShare),
            empty_section("d", SectionSize::EqualShare),
        ]);
        let metrics = MsvLayoutMetrics {
            cell_quantum: 1.0,
            ..MsvLayoutMetrics::default()
        };
        // 21 rows: 4 headers + 17 body cells, distributed unevenly.
        let layout = v.layout(Rect::new(0.0, 2.0, 30.0, 21.0), metrics, |_| {
            SectionMeasure {
                content_size: 5.0,
                aux_size: 0.0,
            }
        });
        for s in &layout.sections {
            // Paint draws header at this integer row.
            let painted_row = s.header_bounds.y.round();
            // Hit-test at that row's center must return Header for this section.
            let hit = layout.hit_test(15.0, painted_row + 0.0);
            match hit {
                MultiSectionViewHit::Header {
                    section: hit_section,
                    ..
                } => assert_eq!(
                    hit_section, s.section_idx,
                    "row {} paints section {} header but hit_test returns section {}",
                    painted_row, s.section_idx, hit_section
                ),
                other => panic!(
                    "row {} paints section {} header but hit_test returns {:?}",
                    painted_row, s.section_idx, other
                ),
            }
        }
    }

    #[test]
    fn aux_input_hit_returns_input_kind() {
        let mut s = empty_section("sc", SectionSize::EqualShare);
        s.aux = Some(SectionAux::Input(InlineInput {
            id: WidgetId::new("commit"),
            text: String::new(),
            caret: 0,
            placeholder: Some("Commit message".into()),
            has_focus: false,
        }));
        let v = view(vec![s]);
        let layout = v.layout(
            Rect::new(0.0, 0.0, 30.0, 10.0),
            MsvLayoutMetrics::default(),
            |_| SectionMeasure {
                content_size: 0.0,
                aux_size: 1.0,
            },
        );
        // Header at y=0..1, aux at y=1..2, body at y=2..10.
        match layout.hit_test(15.0, 1.5) {
            MultiSectionViewHit::Aux {
                kind: AuxHit::Input,
                section: 0,
            } => {}
            other => panic!("expected Aux::Input, got {:?}", other),
        }
    }
}
