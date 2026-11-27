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
//! # Composition
//!
//! `MultiSectionView` does not reimplement tree, list or form painting.
//! Each section's `body` is a [`SectionBody`] carrying an existing
//! quadraui widget ([`TreeView`], [`ListView`], [`Form`], [`Terminal`],
//! [`MessageList`], [`Chart`]), plain `Text`, an `Empty` welcome state,
//! or a host-painted `Custom` region.
//!
//! # Widget contract
//!
//! [`crate::primitive::Primitive::layout`] resolves the chrome layout
//! ([`MultiSectionView::layout`] with the unit's own metrics: [`pixel_msv_metrics`]
//! on a pixel surface, [`cell_msv_metrics`] on a cell grid) and then lays
//! every expanded, visible body out through that body widget's own
//! `Primitive::layout`. Paint draws the chrome (headers, aux rows,
//! scrollbars, dividers) with this module's painters and each body through
//! its own widget's `paint` / `paint_cells`; the section's gutter owns a
//! tree or list body's scrollbar, so the body does not draw a second one.
//!
//! [`crate::interactive::Interactive`] is controlled: collapse, per-section
//! scroll (a tree or list body's own `scroll_offset`), panel scroll and
//! section sizes stay descriptor fields the app writes back from the
//! [`MultiSectionViewEvent`]s the view reports. Its state holds only the
//! gestures in flight (a divider or thumb drag, aux-input editing) and
//! each body widget's own `Interactive::State`; every event that lands on
//! a body is forwarded to that body widget's `handle`, and what it reports
//! comes back as [`MultiSectionViewEvent::Body`].

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

/// Layout direction for the section stack.
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// What a [`MultiSectionView`] reports through its
/// [`crate::interactive::Interactive`] impl. Every visible change it asks
/// for is the app's to apply: the view keeps collapse, scroll, sizes and
/// aux-input text as descriptor fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum MultiSectionViewEvent {
    /// A header was clicked, or activated from the keyboard (Enter or
    /// Space on its focus stop): its title area, its chevron, or one of
    /// its action buttons.
    HeaderActivated { section: usize, hit: HeaderHit },
    /// A header asked to collapse or expand its section; the app writes
    /// `collapsed` onto [`Section::collapsed`].
    CollapseChanged { section: usize, collapsed: bool },
    /// A divider drag resized the two sections either side of it; the
    /// app writes both sizes as [`SectionSize::Fixed`].
    SectionResized {
        above: usize,
        above_size: u16,
        below: usize,
        below_size: u16,
    },
    /// A scrollbar gesture asked to scroll. `section: Some(i)` is the new
    /// row `scroll_offset` of section `i`'s tree or list body; `None` is
    /// the new [`MultiSectionView::panel_scroll`], in main-axis units.
    ScrollChanged { section: Option<usize>, offset: f32 },
    /// An aux-row toolbar button or a custom aux was clicked.
    AuxActivated { section: usize, hit: AuxHit },
    /// Editing changed an aux input's text or caret; the app writes both
    /// onto its [`InlineInput`] (`caret` in chars, like the field).
    AuxInputChanged {
        section: usize,
        text: String,
        caret: usize,
    },
    /// Enter was pressed in an aux input.
    AuxInputSubmitted { section: usize, text: String },
    /// An event section `.0`'s body widget reported, as that widget's own
    /// [`crate::UiEvent`] under its own id.
    Body(usize, Box<crate::UiEvent>),
}

impl crate::interactive::WidgetAction for MultiSectionViewEvent {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        crate::UiEvent::MultiSectionView(id, self)
    }
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
    /// [`Self::panel_scrollbar`], computed via [`crate::fit_thumb`].
    /// Published here so callers that need the thumb's exact travel
    /// distance for drag read it straight off the layout instead of
    /// re-deriving the thumb-sizing formula themselves.
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
/// `cell_quantum > 0.0` flags TUI: the painter clamps thumb height to
/// 1 cell (`.max(1)` in `paint_panel_scrollbar`), so we mirror that
/// here. GTK keeps the historical 8-pixel floor (clamped up to
/// `scrollbar_size` when the gutter is wider).
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

mod layout;
pub use layout::{
    cell_msv_layout, cell_msv_metrics, msv_body_measure, pixel_msv_layout, pixel_msv_metrics,
};
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub use layout::{MultiSectionViewPrimitiveLayout, SectionBodyLayout, SectionBodyView};

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
mod contract;
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub use contract::{MultiSectionViewState, MultiSectionViewSubPart, SectionBodyState};

#[cfg(feature = "tui")]
pub(crate) mod paint_cells;

/// The shared pixel painter for [`MultiSectionView`]: [`paint`] draws the
/// whole view from its contract layout — chrome through this module's
/// helpers, each body through its own widget's
/// [`crate::primitive::Primitive::paint`].
///
/// Every helper brackets the header title in
/// [`PaintSurface::surface_push_clip`]/[`PaintSurface::surface_pop_clip`]
/// so a long title never bleeds past the first action slot, and both
/// scrollbar painters consume the thumb bounds the layout already
/// resolved rather than re-deriving them.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) mod native_surface_paint {
    use super::contract::with_body_paint;
    use super::{
        Axis, EmptyBody, HeaderAction, MultiSectionView, MultiSectionViewPrimitiveLayout,
        SectionAux, SectionBody, SectionBodyLayout, SectionBodyView, SectionHeader,
    };
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::primitive::Primitive;
    use crate::primitives::toolbar::action_text;
    use crate::theme::Theme;
    use crate::types::{Color, StyledText};
    use crate::PaintEnv;

    /// Paint `view` at `layout` (its contract layout) onto `surface`:
    /// the panel background, then per section its header (on whole-pixel
    /// rows), aux row, body and scrollbar gutter, then the dividers, all
    /// clipped to the panel; the panel scrollbar paints last, unclipped.
    /// Each body paints through its own widget inside its visible rect;
    /// `Text` and `Empty` bodies use this module's painters.
    /// `caret_visible` gates a focused aux input's caret, for a caller
    /// with its own blink phase.
    pub(crate) fn paint(
        view: &MultiSectionView,
        layout: &MultiSectionViewPrimitiveLayout,
        surface: &mut dyn PaintSurface,
        env: &PaintEnv,
        caret_visible: bool,
    ) {
        let theme = env.theme();
        let bounds = layout.chrome.bounds;
        if bounds.width <= 0.0 || bounds.height <= 0.0 || view.axis == Axis::Horizontal {
            return;
        }
        surface.surface_push_clip(bounds);
        surface.surface_fill_rect(bounds, theme.background);
        for s in &layout.chrome.sections {
            let section = &view.sections[s.section_idx];
            let header = Rect::new(
                s.header_bounds.x,
                s.header_bounds.y.round(),
                s.header_bounds.width,
                s.header_bounds.height.round(),
            );
            paint_header(surface, header, &section.header, section.collapsed, theme);
            if s.collapsed {
                continue;
            }
            if let (Some(aux), Some(aux_b)) = (&section.aux, s.aux_bounds) {
                paint_aux(surface, aux_b, aux, theme, caret_visible);
            }
            if let Some(body) = layout.body(s.section_idx) {
                surface.surface_push_clip(body.clip);
                paint_body(view, s.section_idx, body, surface, env, layout.line_height);
                surface.surface_pop_clip();
            }
            if let Some(gutter) = s.scrollbar_bounds {
                paint_scrollbar(surface, gutter, s.thumb_bounds, theme);
            }
        }
        if view.allow_resize {
            for d in &layout.chrome.dividers {
                paint_divider(surface, d.bounds, theme);
            }
        }
        surface.surface_pop_clip();
        if let Some(panel) = layout.chrome.panel_scrollbar {
            paint_panel_scrollbar(surface, panel, layout.chrome.panel_scrollbar_thumb, theme);
        }
    }

    fn paint_body(
        view: &MultiSectionView,
        idx: usize,
        body: &SectionBodyView,
        surface: &mut dyn PaintSurface,
        env: &PaintEnv,
        line_height: f32,
    ) {
        let theme = env.theme();
        let descriptor = body.body.as_ref().unwrap_or(&view.sections[idx].body);
        match (descriptor, &body.layout) {
            (SectionBody::Text(lines), SectionBodyLayout::Text(rect)) => {
                paint_text_lines(surface, *rect, lines, theme, line_height);
            }
            (SectionBody::Empty(empty), SectionBodyLayout::Empty(rect)) => {
                paint_empty_body(surface, *rect, empty, theme, line_height);
            }
            _ => {
                let interaction = view.body_interaction(idx, env);
                let child_env = view.body_env(idx, env, &interaction);
                with_body_paint!(descriptor, &body.layout, |w, l| Primitive::paint(
                    w, l, surface, &child_env
                ));
            }
        }
    }

    fn plain_text(t: &StyledText) -> String {
        t.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// Glyph text + resolved foreground for a [`HeaderAction`]'s button.
    /// `action_text`, not a hand-rolled format, so the glyph composes the
    /// same way every other action button in this crate does — even
    /// though a `HeaderAction` only ever supplies the `label` half (no
    /// separate icon or key hint).
    fn header_action_glyph_and_fg(
        action: &HeaderAction,
        theme: &Theme,
        enabled_fg: Color,
    ) -> (String, Color) {
        let glyph = action_text(&action.icon.fallback, None, None);
        let fg = if action.enabled {
            enabled_fg
        } else {
            theme.muted_fg
        };
        (glyph, fg)
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
            // Same codicon chevron `TreeView`'s own GUI paint substitutes
            // for its default `▾`/`▸` (`crate::primitives::tree::tree_chevron_glyph`),
            // so a section header reads as the same control as the tree
            // it expands/collapses, not a mismatched plain-text one.
            let chevron = if collapsed {
                crate::codicon::CHEVRON_RIGHT.to_string()
            } else {
                crate::codicon::CHEVRON_DOWN.to_string()
            };
            let (cw, ch) = surface.surface_measure_text(&chevron);
            surface.surface_draw_text_run(
                Rect::new(left_x, row_text_y(ch), cw, ch),
                &chevron,
                theme.header_fg,
            );
            left_x += cw + 4.0;
        }

        // Right-aligned actions, right-to-left.
        let mut right_x = bounds.x + bounds.width - 4.0;
        for action in header.actions.iter().rev() {
            let (glyph, action_fg) = header_action_glyph_and_fg(action, theme, theme.header_fg);
            let (gw, gh) = surface.surface_measure_text(&glyph);
            right_x -= gw;
            if right_x < left_x {
                break;
            }
            surface.surface_draw_text_run(
                Rect::new(right_x, row_text_y(gh), gw, gh),
                &glyph,
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
    /// `caret_visible` lets a caller with its own blink-timer state gate the
    /// caret paint; a caller without one passes `true` unconditionally.
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
                    let (glyph, action_fg) = header_action_glyph_and_fg(a, theme, theme.foreground);
                    let (gw, gh) = surface.surface_measure_text(&glyph);
                    surface.surface_draw_text_run(
                        Rect::new(tx, bounds.y + (bounds.height - gh) / 2.0, gw, gh),
                        &glyph,
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

        // The action block paints as a bordered button (a stroked box
        // around the label), not a `[ label ]` glyph — the TUI painter
        // keeps the brackets. Every other block is plain centred text,
        // so the two shapes are kept distinct rather than forcing a
        // button through the same `(String, Color)` tuple the text
        // blocks use.
        enum EmptyBlock {
            Text(String, Color),
            Button(String),
        }

        let mut blocks: Vec<EmptyBlock> = Vec::new();
        if let Some(icon) = &empty.icon {
            blocks.push(EmptyBlock::Text(icon.fallback.clone(), theme.foreground));
        }
        let primary = plain_text(&empty.text);
        if !primary.is_empty() {
            blocks.push(EmptyBlock::Text(primary, theme.foreground));
        }
        if let Some(hint) = &empty.hint {
            let hint_str = plain_text(hint);
            if !hint_str.is_empty() {
                blocks.push(EmptyBlock::Text(hint_str, theme.muted_fg));
            }
        }
        if let Some(action) = &empty.action {
            let label = action
                .tooltip
                .clone()
                .unwrap_or_else(|| action.icon.fallback.clone());
            blocks.push(EmptyBlock::Button(label));
        }
        if blocks.is_empty() {
            return;
        }

        const BUTTON_PAD_X: f32 = 12.0;

        let total_h = blocks.len() as f32 * line_height;
        let mut block_y = bounds.y + (bounds.height - total_h).max(0.0) / 2.0;
        for block in &blocks {
            match block {
                EmptyBlock::Text(text, color) => {
                    let (tw, th) = surface.surface_measure_text(text);
                    let block_x = bounds.x + (bounds.width - tw).max(0.0) / 2.0;
                    surface.surface_draw_text_run(
                        Rect::new(block_x, block_y + (line_height - th) / 2.0, tw, th),
                        text,
                        *color,
                    );
                }
                EmptyBlock::Button(label) => {
                    let (tw, th) = surface.surface_measure_text(label);
                    let btn_w = tw + BUTTON_PAD_X * 2.0;
                    let btn_h = (line_height - 4.0).max(th + 4.0);
                    let btn_x = bounds.x + (bounds.width - btn_w).max(0.0) / 2.0;
                    let btn_y = block_y + (line_height - btn_h) / 2.0;
                    let btn_rect = Rect::new(btn_x, btn_y, btn_w, btn_h);
                    surface.surface_stroke_rect(btn_rect, theme.accent_fg, 1.0);
                    surface.surface_draw_text_run(
                        Rect::new(btn_x + BUTTON_PAD_X, btn_y + (btn_h - th) / 2.0, tw, th),
                        label,
                        theme.accent_fg,
                    );
                }
            }
            block_y += line_height;
        }
    }

    /// Paint a per-section scrollbar gutter: a 50%-alpha track with a 90%-alpha
    /// thumb on top, both real alpha blends against whatever is already painted
    /// underneath.
    ///
    /// `thumb_bounds` comes from the layout when the body's scroll state is
    /// introspectable (`Tree`, `List`); `None` falls back to a 20%-tall top-anchored
    /// placeholder thumb, preserving the visual for other overflowing body types.
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

    /// Paint the panel-level scrollbar (`ScrollMode::WholePanel`): an opaque
    /// track, then an opaque thumb at `thumb_bounds`. Geometry is computed once
    /// by [`crate::primitives::multi_section_view::MultiSectionView::layout`]
    /// (`fit_thumb`) and published as
    /// [`crate::primitives::multi_section_view::MultiSectionViewLayout::panel_scrollbar_thumb`],
    /// never re-derived here.
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
            /// One entry per `surface_stroke_rect` call.
            strokes: Vec<(Rect, Color, f32)>,
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
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
                self.strokes.push((rect, color, stroke_width));
            }
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
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

        /// The title text draw must always happen while exactly one clip is active
        /// (pushed immediately before, popped immediately after).
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

        /// The leading chevron paints the same codicon glyph `TreeView`'s own
        /// GUI paint path substitutes for its default `▾`/`▸`
        /// (`crate::codicon::CHEVRON_DOWN`/`CHEVRON_RIGHT`), not the plain
        /// Unicode triangle — a GUI section header must match the tree
        /// control it expands/collapses.
        #[test]
        fn paint_header_chevron_is_the_codicon_glyph_not_plain_unicode() {
            let mut header = header_with("Results", vec![]);
            header.show_chevron = true;

            let mut expanded = RecordingSurface::default();
            paint_header(
                &mut expanded,
                Rect::new(0.0, 0.0, 120.0, 20.0),
                &header,
                false,
                &Theme::default(),
            );
            assert!(
                expanded
                    .texts
                    .iter()
                    .any(|(_, text, ..)| text == crate::codicon::CHEVRON_DOWN.to_string().as_str()),
                "expanded header must paint the codicon CHEVRON_DOWN glyph, got {:?}",
                expanded.texts
            );
            assert!(
                !expanded
                    .texts
                    .iter()
                    .any(|(_, text, ..)| text == "\u{25BE}"),
                "expanded header must not paint the plain Unicode ▾, got {:?}",
                expanded.texts
            );

            let mut collapsed = RecordingSurface::default();
            paint_header(
                &mut collapsed,
                Rect::new(0.0, 0.0, 120.0, 20.0),
                &header,
                true,
                &Theme::default(),
            );
            assert!(
                collapsed
                    .texts
                    .iter()
                    .any(|(_, text, ..)| text == crate::codicon::CHEVRON_RIGHT.to_string().as_str()),
                "collapsed header must paint the codicon CHEVRON_RIGHT glyph, got {:?}",
                collapsed.texts
            );
            assert!(
                !collapsed
                    .texts
                    .iter()
                    .any(|(_, text, ..)| text == "\u{25B8}"),
                "collapsed header must not paint the plain Unicode ▸, got {:?}",
                collapsed.texts
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

        /// Per-section scrollbar track and thumb must carry real alpha, never a
        /// fully-opaque colour pre-blended against a hardcoded destination.
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

        /// The panel-level scrollbar must consume the layout-supplied `thumb_bounds`
        /// verbatim rather than re-deriving thumb geometry from `(scroll, total)`.
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

        /// The empty-state action paints as a bordered button — a
        /// `surface_stroke_rect` around the label — not a
        /// `[ label ]` text glyph. Regression guard for the drawn-vs-text
        /// affordance swap.
        #[test]
        fn empty_body_action_paints_a_stroked_button_not_bracket_text() {
            let mut surface = RecordingSurface::default();
            let empty = EmptyBody {
                icon: None,
                text: StyledText::plain("No results"),
                hint: None,
                action: Some(HeaderAction {
                    id: crate::primitives::multi_section_view::ActionId::from("open-folder"),
                    icon: crate::types::Icon::new("", "Open"),
                    tooltip: Some("Open Folder".to_string()),
                    enabled: true,
                }),
            };
            paint_empty_body(
                &mut surface,
                Rect::new(0.0, 0.0, 200.0, 100.0),
                &empty,
                &Theme::default(),
                16.0,
            );

            assert!(
                !surface
                    .texts
                    .iter()
                    .any(|(_, text, _, _)| text.contains('[')),
                "no bracket glyph should be painted for the action button; \
                 texts were {:?}",
                surface.texts,
            );
            assert!(
                surface
                    .texts
                    .iter()
                    .any(|(_, text, _, _)| text == "Open Folder"),
                "the action label itself must still be painted; texts were {:?}",
                surface.texts,
            );
            assert_eq!(
                surface.strokes.len(),
                1,
                "the action button must stroke exactly one bordering rect; \
                 strokes were {:?}",
                surface.strokes,
            );
        }

        fn one_section_view(
            body: crate::primitives::multi_section_view::SectionBody,
        ) -> MultiSectionView {
            MultiSectionView {
                id: WidgetId::new("msv"),
                sections: vec![crate::primitives::multi_section_view::Section {
                    id: "s".into(),
                    header: header_with("S", vec![]),
                    body,
                    aux: None,
                    size: crate::primitives::multi_section_view::SectionSize::EqualShare,
                    collapsed: false,
                    min_size: None,
                    max_size: None,
                }],
                active_section: None,
                axis: Axis::Vertical,
                allow_resize: false,
                allow_collapse: true,
                scroll_mode: Default::default(),
                has_focus: false,
                panel_scroll: 0.0,
            }
        }

        fn paint_view(view: &MultiSectionView, bounds: Rect) -> RecordingSurface {
            let ctx = crate::primitive::LayoutCtx::new(16.0, 8.0, false);
            let layout = Primitive::layout(view, &ctx, bounds);
            let (theme, style) = (Theme::default(), crate::Style::default());
            let mut surface = RecordingSurface::default();
            paint(
                view,
                &layout,
                &mut surface,
                &PaintEnv::new(&theme, &style),
                true,
            );
            surface
        }

        /// A message-list section paints its rows through the message
        /// list's own painter on every pixel backend.
        #[test]
        fn message_list_body_paints_its_rows() {
            let list = crate::primitives::message_list::MessageList {
                id: WidgetId::new("msgs"),
                rows: vec![crate::primitives::message_list::MessageRow::new(
                    "hello there",
                    Color::rgb(200, 200, 200),
                    0.0,
                )],
                scroll_top: 0,
            };
            let view = one_section_view(SectionBody::MessageList(list));
            let surface = paint_view(&view, Rect::new(0.0, 0.0, 200.0, 100.0));
            assert!(
                surface
                    .texts
                    .iter()
                    .any(|(_, t, ..)| t.contains("hello there")),
                "{:?}",
                surface.texts
            );
        }

        /// An overflowing tree section shows one scrollbar: the section's
        /// gutter, with the tree's own scrollbar handed to it.
        #[test]
        fn overflowing_tree_section_paints_one_scrollbar() {
            let rows = (0..40)
                .map(|i| crate::primitives::tree::TreeRow {
                    path: vec![i],
                    indent: 0,
                    icon: None,
                    text: StyledText::plain(format!("row{i}")),
                    badge: None,
                    is_expanded: None,
                    decoration: Default::default(),
                    edit: None,
                })
                .collect();
            let tree = crate::primitives::tree::TreeView {
                id: WidgetId::new("t"),
                rows,
                selection_mode: crate::types::SelectionMode::Single,
                selected_path: None,
                scroll_offset: 0,
                style: Default::default(),
                has_focus: false,
            };
            assert!(tree.style.show_scrollbar);
            let view = one_section_view(SectionBody::Tree(tree));
            let bounds = Rect::new(0.0, 0.0, 200.0, 120.0);
            let ctx = crate::primitive::LayoutCtx::new(16.0, 8.0, false);
            let layout = Primitive::layout(&view, &ctx, bounds);
            let gutter = layout.chrome.sections[0]
                .scrollbar_bounds
                .expect("40 rows overflow the section");
            let body = layout.body(0).expect("an expanded tree body");
            match (&body.body, &body.layout) {
                (Some(SectionBody::Tree(t)), SectionBodyLayout::Tree(l)) => {
                    assert!(!t.style.show_scrollbar);
                    assert!(l.vscrollbar.is_none());
                }
                other => panic!("expected the gutter to own the tree scrollbar, got {other:?}"),
            }
            let theme = Theme::default();
            let surface = paint_view(&view, bounds);
            let thumb_fills = surface
                .fills
                .iter()
                .filter(|(_, c)| {
                    *c == theme.scrollbar_thumb.with_alpha(0.9) || *c == theme.scrollbar_thumb
                })
                .collect::<Vec<_>>();
            assert!(
                thumb_fills.iter().all(|(r, _)| r.x >= gutter.x),
                "every scrollbar thumb paints in the gutter: {thumb_fills:?}"
            );
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
        // With a fractional EqualShare distribution (4 sections in 21 cells → 5.25
        // each), paint snaps via bounds.y.round() while hit_test used to consume raw
        // f32 bounds. With cell_quantum: 1.0 the layout itself snaps to whole cells.
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
