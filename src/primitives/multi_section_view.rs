//! `MultiSectionView` primitive: vertically stacked, individually sized,
//! collapsible sections — each containing its own scrollable body widget.
//!
//! Used for VSCode-style side panels (Explorer's Open Editors / Folder /
//! Outline / Timeline), Source Control panels, Debug sidebars, k8s resource
//! browsers, Postman collection sidebars — anything that's "N stacked
//! sections, each its own little scrollable list, with chrome on top."
//!
//! See `quadraui/docs/DECISIONS.md` D-003 for the design pass that
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
    /// Per-section scrollbar bounds (only present in
    /// [`ScrollMode::PerSection`] when the body overflows).
    pub scrollbar_bounds: Option<Rect>,
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutMetrics {
    /// Header row size in main-axis units (e.g. 1 cell, or
    /// `line_height` pixels).
    pub header_size: f32,
    /// Divider stripe size in main-axis units (0 means no divider strip
    /// drawn between sections).
    pub divider_size: f32,
    /// Scrollbar gutter size in cross-axis units. Reserved on the
    /// trailing edge of each section's body when the body overflows.
    pub scrollbar_size: f32,
}

impl Default for LayoutMetrics {
    fn default() -> Self {
        Self {
            header_size: 1.0,
            divider_size: 0.0,
            scrollbar_size: 1.0,
        }
    }
}

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
        metrics: LayoutMetrics,
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
                    hit_regions: Vec::new(),
                }
            }
        }
    }

    fn layout_vertical<F>(
        &self,
        bounds: Rect,
        metrics: LayoutMetrics,
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
        let resolved = match self.scroll_mode {
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

        // ── Walk and emit per-section layouts + dividers ───────────
        let mut sections_out = Vec::with_capacity(n);
        let mut dividers = Vec::new();
        let mut hit_regions: Vec<(Rect, MultiSectionViewHit)> = Vec::new();

        // For WholePanel mode, sections stack at content size and the
        // viewport scrolls the whole panel. `panel_scroll` shifts every
        // section's y upward — sections above `panel_scroll` get
        // negative-y bounds (off-screen above), sections past
        // `bounds.y + bounds.height` get bounds past the viewport
        // bottom. Backends' clip regions handle the visible window.
        let scroll_offset = match self.scroll_mode {
            ScrollMode::WholePanel => self.panel_scroll.max(0.0),
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

            let header_bounds = Rect::new(bounds.x, s_top, bounds.width, metrics.header_size);

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
            let body_w = (bounds.width - scrollbar_w).max(0.0);

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

            sections_out.push(SectionLayout {
                section_idx: i,
                header_bounds,
                aux_bounds,
                body_bounds,
                scrollbar_bounds,
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

            // Scrollbar hit regions: thumb / track-before / track-after.
            // Thumb position is the host's responsibility (we only know
            // total content size; thumb position depends on scroll
            // offset inside the inner body). We emit a single
            // scrollbar-thumb region for now; backends that want
            // track-page-jump can split the bounds further at hit time.
            if let Some(sb) = scrollbar_bounds {
                hit_regions.push((
                    sb,
                    MultiSectionViewHit::Scrollbar {
                        section: i,
                        kind: ScrollbarHit::Thumb,
                    },
                ));
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

        // Panel-level scrollbar (WholePanel mode only).
        let panel_scrollbar = match self.scroll_mode {
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
                    hit_regions.push((
                        r,
                        MultiSectionViewHit::PanelScrollbar {
                            kind: ScrollbarHit::Thumb,
                        },
                    ));
                    Some(r)
                } else {
                    None
                }
            }
            ScrollMode::PerSection => None,
        };

        MultiSectionViewLayout {
            bounds,
            axis: Axis::Vertical,
            scroll_mode: self.scroll_mode,
            sections: sections_out,
            dividers,
            panel_scrollbar,
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

fn body_overflows(s: &Section, content: f32, body_main: f32) -> bool {
    // Empty/Custom never claim overflow — host paints whatever it wants.
    match &s.body {
        SectionBody::Empty(_) | SectionBody::Custom(_) => false,
        _ => content > body_main + 0.5, // tolerate sub-cell rounding
    }
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
            LayoutMetrics::default(),
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
            LayoutMetrics::default(),
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
            LayoutMetrics::default(),
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
            LayoutMetrics::default(),
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
            LayoutMetrics::default(),
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
            LayoutMetrics::default(),
            |_| SectionMeasure::default(),
        );
        // Without min: both 10. With min(a)=15: a is at least 15.
        assert!(layout.sections[0].resolved_size >= 15.0 - 0.01);
    }

    #[test]
    fn header_hit_chevron_vs_title_vs_action() {
        let mut header = SectionHeader::default();
        header.title = StyledText::plain("Section");
        header.show_chevron = true;
        header.actions = vec![HeaderAction {
            id: "refresh".into(),
            icon: Icon::new("", "R"),
            tooltip: None,
            enabled: true,
        }];
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
            LayoutMetrics::default(),
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
        let mut header = SectionHeader::default();
        header.title = StyledText::plain("Section");
        header.show_chevron = false;
        header.actions = vec![HeaderAction {
            id: "noop".into(),
            icon: Icon::new("", "x"),
            tooltip: None,
            enabled: false,
        }];
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
            LayoutMetrics::default(),
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
            LayoutMetrics::default(),
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
            LayoutMetrics::default(),
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
        let metrics = LayoutMetrics {
            divider_size: 1.0,
            ..LayoutMetrics::default()
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
            LayoutMetrics::default(),
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
            LayoutMetrics::default(),
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
