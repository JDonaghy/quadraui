//! `Completions` primitive: an LSP-style autocomplete popup anchored to
//! the cursor position. Similar to `ListView` but tailored for
//! completion semantics — per-item `kind` icon (function / variable /
//! snippet / etc.), right-aligned type detail, optional
//! documentation preview.
//!
//! Unlike `Palette`, the query isn't inside the primitive — it's
//! implicit in the cursor position. The app filters its source against
//! the current word-under-cursor and passes the visible `items`.
//!
//! # Backend contract
//!
//! **Declarative + anchor-positioned.** Apps pass the cursor position
//! and the primitive's `layout()` picks a position that keeps the
//! popup inside the viewport (prefer below the cursor, flip to above
//! if it would overflow). Click on item → `ItemActivated { idx }`;
//! keyboard navigation emits the respective events; Escape / Enter /
//! Tab follow their typical IDE semantics.

use crate::a11y::A11yAction;
use crate::behaviour::nav::{FlatNav, NavKey};
use crate::behaviour::scroll::ensure_visible;
use crate::event::{Key, MouseButton, NamedKey, Rect};
use crate::interactive::{
    EventCx, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget, WidgetAction,
};
use crate::layout::{Anchor, ResolvedSide, Side};
use crate::primitive::{LayoutCtx, Primitive, Size, Units};
use crate::types::{Icon, StyledText, WidgetId};
use crate::Handled;
use serde::{Deserialize, Serialize};

/// Declarative description of a completion popup.
///
/// Stays a plain, non-`#[non_exhaustive]` struct: a published consumer
/// builds it with an exhaustive struct literal today, so marking it
/// `#[non_exhaustive]` would break that consumer's build. `Default` is
/// still provided for consumers that want the
/// `Default::default()`-plus-field-assignment pattern.
///
/// # Examples
///
/// ```
/// use quadraui::{
///     CompletionItem, CompletionItemMeasure, Completions, CompletionsHit, Rect, StyledText,
///     WidgetId,
/// };
///
/// let mut popup = Completions::default();
/// popup.id = WidgetId::new("completions:editor");
/// popup.items = vec![CompletionItem {
///     label: StyledText::plain("map"),
///     detail: None,
///     documentation: None,
///     kind: Default::default(),
///     icon: None,
/// }];
/// popup.has_focus = true;
///
/// let viewport = Rect::new(0.0, 0.0, 80.0, 24.0);
/// let layout = popup.layout(10.0, 5.0, 1.0, viewport, 20.0, 10.0, |_| {
///     CompletionItemMeasure::new(1.0)
/// });
///
/// assert_eq!(layout.hit_test(10.0, 6.0), CompletionsHit::Item(0));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Completions {
    pub id: WidgetId,
    pub items: Vec<CompletionItem>,
    pub selected_idx: usize,
    #[serde(default)]
    pub scroll_offset: usize,
    /// When true, the primitive is "live" and receiving focus for
    /// keyboard nav. When false it's suppressed (rendered but
    /// non-interactive) — apps set this based on their own state.
    #[serde(default)]
    pub has_focus: bool,
}

impl Default for Completions {
    /// An empty, unfocused completion popup with no items. Set fields
    /// directly afterwards (every field is `pub`) — see
    /// `docs/PRIMITIVE_RULES.md` rule 8.
    fn default() -> Self {
        Completions {
            id: WidgetId::new(""),
            items: Vec::new(),
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: false,
        }
    }
}

/// One completion candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionItem {
    /// Display label (what the user sees in the popup). Usually the
    /// same as the insertion text, but LSP can differ (e.g. label
    /// `"map(..)"` inserts `"map"`).
    pub label: StyledText,
    /// Right-aligned type / detail text, e.g. `"fn(T) -> U"` or
    /// `"crate::foo"`.
    #[serde(default)]
    pub detail: Option<StyledText>,
    /// Optional documentation snippet shown in a preview panel. The
    /// primitive doesn't render this — apps display it separately when
    /// they care.
    #[serde(default)]
    pub documentation: Option<StyledText>,
    /// Kind icon (LSP CompletionItemKind mapping).
    #[serde(default)]
    pub kind: CompletionKind,
    /// Icon override. Backends default to a theme-defined glyph per
    /// `kind` if this is `None`.
    #[serde(default)]
    pub icon: Option<Icon>,
}

impl CompletionItem {
    /// The icon a rasteriser actually paints: [`Self::icon`] when set,
    /// else [`CompletionKind::default_icon`] for [`Self::kind`]. Every
    /// completion paints *some* icon — `kind` always has a default —
    /// so callers never need their own `Option` fallback.
    pub fn resolved_icon(&self) -> Icon {
        self.icon
            .clone()
            .unwrap_or_else(|| self.kind.default_icon())
    }
}

/// Kind of completion — drives the default icon + colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CompletionKind {
    #[default]
    Text,
    Method,
    Function,
    Constructor,
    Field,
    Variable,
    Class,
    Interface,
    Module,
    Property,
    Unit,
    Value,
    Enum,
    Keyword,
    Snippet,
    Color,
    File,
    Reference,
    Folder,
    EnumMember,
    Constant,
    Struct,
    Event,
    Operator,
    TypeParameter,
}

impl CompletionKind {
    /// Two-letter ASCII mnemonic painted ahead of the label when
    /// [`CompletionItem::icon`] is `None`.
    ///
    /// Not a bundled-codicon glyph: codicon only ships a handful of
    /// `symbol-*` names in [`crate::Icon::codicon`]'s curated table (see
    /// `codicon_names.rs`), nowhere near this enum's 25 variants, and
    /// guessing the rest of the upstream PUA codepoints by hand risks
    /// painting the wrong glyph with nothing to catch it (every real
    /// entry in that table is independently verified against the
    /// bundled font's own `cmap`/`post` tables). An ASCII mnemonic has
    /// no such risk and, with [`Icon::glyph`] equal to [`Icon::fallback`],
    /// paints identically on every backend with no font dependency —
    /// extending the curated codicon table to cover every kind is
    /// follow-on work, not a precondition for painting *an* icon here.
    pub fn default_icon(&self) -> Icon {
        let mnemonic = match self {
            CompletionKind::Text => "tx",
            CompletionKind::Method => "me",
            CompletionKind::Function => "fn",
            CompletionKind::Constructor => "ct",
            CompletionKind::Field => "fd",
            CompletionKind::Variable => "va",
            CompletionKind::Class => "cl",
            CompletionKind::Interface => "if",
            CompletionKind::Module => "mo",
            CompletionKind::Property => "pr",
            CompletionKind::Unit => "un",
            CompletionKind::Value => "vl",
            CompletionKind::Enum => "en",
            CompletionKind::Keyword => "kw",
            CompletionKind::Snippet => "sn",
            CompletionKind::Color => "co",
            CompletionKind::File => "fi",
            CompletionKind::Reference => "rf",
            CompletionKind::Folder => "fo",
            CompletionKind::EnumMember => "em",
            CompletionKind::Constant => "cn",
            CompletionKind::Struct => "st",
            CompletionKind::Event => "ev",
            CompletionKind::Operator => "op",
            CompletionKind::TypeParameter => "tp",
        };
        Icon::new(mnemonic, mnemonic)
    }
}

// ── D6 Layout API ───────────────────────────────────────────────────────────

/// Per-item measurement supplied by the backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompletionItemMeasure {
    pub height: f32,
}

impl CompletionItemMeasure {
    pub fn new(height: f32) -> Self {
        Self { height }
    }
}

/// Resolved position of one visible completion item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleCompletion {
    pub item_idx: usize,
    pub bounds: Rect,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionsHit {
    Item(usize),
    /// Click inside the popup but not on an item (border padding).
    Inert,
    /// Click outside the popup — typically dismisses.
    Empty,
}

/// Resolved placement direction for the popup.
///
/// When the popup's desired height fits on neither side, this reports
/// the *preferred* side (`Below`) rather than whichever side the box
/// was actually pinned against — the box is still clamped fully inside
/// the viewport, just not necessarily on the side this value names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionsPlacement {
    /// Popup rendered below the cursor anchor.
    Below,
    /// Popup rendered above the cursor anchor (flipped to avoid
    /// overflow).
    Above,
}

/// Fully-resolved completions-popup layout.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionsLayout {
    pub bounds: Rect,
    pub placement: CompletionsPlacement,
    pub visible_items: Vec<VisibleCompletion>,
    pub hit_regions: Vec<(Rect, CompletionsHit)>,
    pub resolved_scroll_offset: usize,
}

impl CompletionsLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> CompletionsHit {
        let inside = x >= self.bounds.x
            && x < self.bounds.x + self.bounds.width
            && y >= self.bounds.y
            && y < self.bounds.y + self.bounds.height;
        if !inside {
            return CompletionsHit::Empty;
        }
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        CompletionsHit::Inert
    }
}

impl Completions {
    /// Compute popup placement + per-item bounds.
    ///
    /// # Arguments
    ///
    /// - `cursor_x`, `cursor_y` — caret position in viewport coords.
    /// - `line_height` — height of the line containing the cursor
    ///   (used for the gap between cursor and popup).
    /// - `viewport` — parent surface bounds.
    /// - `popup_width`, `max_popup_height` — size constraints for the
    ///   popup.
    /// - `measure_item(i)` — height for item `i`.
    ///
    /// # Placement
    ///
    /// Prefer below the cursor; flip above if below would overflow.
    /// Shift left if the popup would extend past the right edge.
    #[allow(clippy::too_many_arguments)]
    pub fn layout<F>(
        &self,
        cursor_x: f32,
        cursor_y: f32,
        line_height: f32,
        viewport: Rect,
        popup_width: f32,
        max_popup_height: f32,
        measure_item: F,
    ) -> CompletionsLayout
    where
        F: Fn(usize) -> CompletionItemMeasure,
    {
        // Clamp scroll offset.
        let resolved_scroll_offset =
            crate::primitives::scrollbar::clamp_scroll_offset(self.scroll_offset, self.items.len());

        // Determine the popup's height (bounded by max and total content).
        let total_content: f32 = (resolved_scroll_offset..self.items.len())
            .map(|i| measure_item(i).height)
            .sum();
        let desired_height = total_content.min(max_popup_height);

        // Horizontal position is clamped into the viewport *before*
        // `Anchor::resolve` runs, independently of the vertical flip
        // decision below. `Anchor::resolve`'s `fits` predicate tests
        // both axes together, so an anchor `x` that is already outside
        // the viewport would make the horizontal overflow fail the fit
        // test on whichever vertical side is tried, masking the
        // intended vertical flip. Pre-clamping keeps the two axes
        // independent, matching this popup's horizontal "shift left on
        // overflow" contract.
        let ax = cursor_x.clamp(
            viewport.x,
            (viewport.x + viewport.width - popup_width).max(viewport.x),
        );

        // Vertical placement: below cursor if it fits, else above, via
        // the shared `Anchor` flip/clamp resolver. The anchor rect's
        // width is set to `popup_width` so `Anchor::resolve`'s cross-axis
        // centering (`(rect.width - w) * 0.5`) collapses to zero and `x`
        // stays left-aligned with `ax` rather than centered; its height
        // is `line_height` so the `Below` candidate opens flush under
        // the cursor's line.
        //
        // When *neither* side fits vertically, `Anchor::resolve` pins to
        // the preferred side (`Below`) and clamps, rather than picking
        // whichever side has more room — see
        // `completions_layout_pins_below_when_neither_side_fully_fits`.
        let anchor_rect = Rect::new(ax, cursor_y, popup_width, line_height);
        let (origin, resolved_side) =
            Anchor::new(anchor_rect, Side::Bottom).resolve((popup_width, desired_height), viewport);
        let (x, y) = (origin.x, origin.y);
        let placement = match resolved_side {
            ResolvedSide::Bottom => CompletionsPlacement::Below,
            ResolvedSide::Top => CompletionsPlacement::Above,
            ResolvedSide::Left | ResolvedSide::Right | ResolvedSide::AtPoint => {
                unreachable!("Completions only ever prefers Side::Bottom")
            }
        };

        // Clip popup to the viewport.
        let clipped_h = (viewport.y + viewport.height - y)
            .min(desired_height)
            .max(0.0);
        let bounds = Rect::new(x, y, popup_width, clipped_h);

        self.resolve_at(bounds, placement, resolved_scroll_offset, measure_item)
    }

    /// Resolve per-item bounds and hit regions for a popup box already
    /// placed at `bounds`, starting at item `resolved_scroll_offset` and
    /// reporting `placement` as the resolved side.
    ///
    /// The shared core of [`Self::layout`] — which picks `bounds` against
    /// a cursor anchor first — and
    /// [`Primitive::layout`], which
    /// is handed the already-placed box by its caller. Rows stop at the
    /// box's bottom edge; the last one is clipped to whatever height is
    /// left rather than overflowing it.
    fn resolve_at<F>(
        &self,
        bounds: Rect,
        placement: CompletionsPlacement,
        resolved_scroll_offset: usize,
        measure_item: F,
    ) -> CompletionsLayout
    where
        F: Fn(usize) -> CompletionItemMeasure,
    {
        let mut visible_items: Vec<VisibleCompletion> = Vec::new();
        let mut hit_regions: Vec<(Rect, CompletionsHit)> = Vec::new();

        let bottom = bounds.y + bounds.height;
        let mut cursor_row = bounds.y;
        for i in resolved_scroll_offset..self.items.len() {
            if cursor_row >= bottom {
                break;
            }
            let m = measure_item(i);
            let remaining = bottom - cursor_row;
            let draw_h = m.height.min(remaining).max(0.0);
            if draw_h <= 0.0 {
                break;
            }
            let item_bounds = Rect::new(bounds.x, cursor_row, bounds.width, draw_h);
            visible_items.push(VisibleCompletion {
                item_idx: i,
                bounds: item_bounds,
            });
            hit_regions.push((item_bounds, CompletionsHit::Item(i)));
            cursor_row += m.height;
        }

        CompletionsLayout {
            bounds,
            placement,
            visible_items,
            hit_regions,
            resolved_scroll_offset,
        }
    }
}

// ── Widget contract: `Primitive` + `Interactive` ───────────────────────────

/// The semantic outcomes a [`Completions`] popup reports — its
/// [`Interactive::Action`].
///
/// [`Completions`] stays purely declarative: no variant below mutates the
/// popup. Each reports the state the host should write back onto its own
/// `Completions` (or, for [`Self::Dismissed`], that it should drop the
/// popup) before the next frame, the same contract
/// [`crate::ListViewEvent`] carries for a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionsEvent {
    /// Keyboard navigation, or a single click on a row, moved the
    /// highlight to `idx`. The host writes it onto
    /// [`Completions::selected_idx`]; the next
    /// [`Primitive::layout`] scrolls
    /// it back into view on its own, so there is no companion
    /// scroll-offset event to apply.
    SelectionChanged { idx: usize },
    /// Enter, Tab, a double-click, or an assistive-technology
    /// [`A11yAction::Click`] accepted `idx` — the host inserts that
    /// candidate and closes the popup.
    Accepted { idx: usize },
    /// Escape dismissed the popup without accepting anything.
    Dismissed,
}

impl WidgetAction for CompletionsEvent {
    /// [`Self::Accepted`] is the one completion outcome the crate's
    /// [`crate::UiEvent`] vocabulary names directly
    /// ([`crate::UiEvent::CompletionItemActivated`]).
    ///
    /// The other two ride [`crate::UiEvent::List`] under the popup's own
    /// [`Completions::id`]: a moved highlight is exactly
    /// [`crate::ListViewEvent::SelectionChanged`]'s `{ idx }` payload, and
    /// a dismissal is the Escape keystroke the popup saw but did not act
    /// on itself, which is what
    /// [`crate::ListViewEvent::KeyPressed`] carries. Reusing that channel
    /// keeps both outcomes loss-free under one id instead of mapping them
    /// onto an unrelated event an existing handler could act on.
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            CompletionsEvent::Accepted { idx } => crate::UiEvent::CompletionItemActivated(id, idx),
            CompletionsEvent::SelectionChanged { idx } => {
                crate::UiEvent::List(id, crate::ListViewEvent::SelectionChanged { idx })
            }
            CompletionsEvent::Dismissed => crate::UiEvent::List(
                id,
                crate::ListViewEvent::KeyPressed {
                    key: NamedKey::Escape.name().into_owned(),
                    modifiers: crate::Modifiers::default(),
                },
            ),
        }
    }
}

/// [`Completions`]'s fully-resolved widget-contract layout: the absolute
/// `area` the popup was laid out in, the metrics it was resolved against,
/// and the popup-local [`CompletionsLayout`] itself.
///
/// `#[non_exhaustive]`: produced by
/// [`Primitive::layout`], never built
/// by a consumer via struct literal — same convention
/// [`CompletionsLayout`] itself follows.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct CompletionsPrimitiveLayout {
    /// The rect [`Primitive::layout`]
    /// was called with — the popup's whole box, already placed by the
    /// host.
    pub area: Rect,
    /// Row pitch the rows were resolved at, from
    /// [`crate::LayoutCtx::line_height`].
    pub row_height: f32,
    /// Nerd-Font toggle resolved the same way — which of
    /// [`Icon::glyph`]/[`Icon::fallback`] the pixel painter paints.
    pub nerd_fonts_enabled: bool,
    /// The resolved popup layout. Its `bounds`/`visible_items`/
    /// `hit_regions` are in the same absolute space as [`Self::area`].
    pub layout: CompletionsLayout,
}

impl CompletionsPrimitiveLayout {
    /// Hit-test at a position relative to [`Self::area`]'s own origin —
    /// the space [`RoutedEvent::Pointer`]'s `local` field uses.
    pub fn hit_test_local(&self, x: f32, y: f32) -> CompletionsHit {
        self.layout.hit_test(self.area.x + x, self.area.y + y)
    }

    /// How many rows the box shows at once — the page size
    /// `PageUp`/`PageDown` move by. At least `1`, so a box too short for
    /// a single row still pages one item at a time rather than standing
    /// still.
    pub fn page(&self) -> usize {
        self.layout.visible_items.len().max(1)
    }
}

impl Completions {
    /// The widest row's content, in the units `ctx` measures in — the
    /// kind icon, a one-advance gap, the label, and (when present) a gap
    /// plus the right-aligned `detail`.
    fn widest_row(&self, ctx: &LayoutCtx) -> f32 {
        let cells = matches!(ctx.units(), Units::Cells);
        let width_of = |text: &str| -> f32 {
            if cells {
                crate::text_util::display_width(text) as f32
            } else {
                ctx.text_width(text, crate::FontRole::Chrome)
            }
        };
        let gap = if cells { 1.0 } else { ctx.char_width };
        self.items
            .iter()
            .map(|item| {
                let icon = item.resolved_icon();
                let glyph = if ctx.nerd_fonts_enabled && !cells {
                    icon.glyph
                } else {
                    icon.fallback
                };
                let label: f32 = item.label.spans.iter().map(|s| width_of(&s.text)).sum();
                let detail = item
                    .detail
                    .as_ref()
                    .map(|d| gap + d.spans.iter().map(|s| width_of(&s.text)).sum::<f32>())
                    .unwrap_or(0.0);
                width_of(&glyph) + gap + label + detail
            })
            .fold(0.0_f32, f32::max)
    }
}

impl Primitive for Completions {
    type Layout = CompletionsPrimitiveLayout;
    type Event = CompletionsEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Completions(self.clone())
    }

    /// One row per candidate, plus the two side-border columns the
    /// rasterisers frame each row with and the widest row's own content
    /// ([`Self::widest_row`]).
    fn measure(&self, ctx: &LayoutCtx, _avail: Size) -> crate::NaturalSize {
        let rows = self.items.len() as f32;
        let chrome = match ctx.units() {
            Units::Cells => 2.0,
            _ => 2.0 * ctx.char_width + ctx.style().row_padding_x,
        };
        crate::NaturalSize::new(
            Some(self.widest_row(ctx) + chrome),
            Some(rows * ctx.line_height),
        )
    }

    /// `rect` is the popup's final box — the `bounds` a host already
    /// resolved through [`Completions::layout`]'s cursor-anchored
    /// placement — so this reports [`CompletionsPlacement::Below`], the
    /// preferred (unflipped) side, the same way an already-placed
    /// [`crate::Tooltip`] reports its preferred placement.
    ///
    /// Rows are `ctx.line_height` tall. The scroll offset is clamped and
    /// then run through [`ensure_visible`], so moving
    /// [`Completions::selected_idx`] past the window's edge scrolls the
    /// highlighted candidate back into view without the host tracking a
    /// second scroll field — the same derivation
    /// [`crate::Palette`]'s own layout applies.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> CompletionsPrimitiveLayout {
        let row_height = if ctx.line_height > 0.0 {
            ctx.line_height
        } else {
            1.0
        };
        let rows = (rect.height / row_height).floor().max(0.0) as usize;
        let clamped =
            crate::primitives::scrollbar::clamp_scroll_offset(self.scroll_offset, self.items.len());
        let selected = self.selected_idx.min(self.items.len().saturating_sub(1));
        let max_scroll = self.items.len().saturating_sub(rows.max(1));
        let resolved = ensure_visible(clamped, rows, selected).min(max_scroll);
        let layout = self.resolve_at(rect, CompletionsPlacement::Below, resolved, |_| {
            CompletionItemMeasure::new(row_height)
        });
        CompletionsPrimitiveLayout {
            area: rect,
            row_height,
            nerd_fonts_enabled: ctx.nerd_fonts_enabled,
            layout,
        }
    }

    /// The shared pixel painter — the same
    /// [`native_surface_paint::paint`] every pixel backend's
    /// `draw_completions` wrapper calls.
    fn paint(
        &self,
        layout: &CompletionsPrimitiveLayout,
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
            native_surface_paint::paint(
                self,
                &layout.layout,
                surface,
                theme,
                style,
                layout.nerd_fonts_enabled,
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
    /// [`crate::tui::draw_completions`] because the `Buffer` →
    /// [`crate::CellSurface`] adapter is private to `crate::tui`.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &CompletionsPrimitiveLayout,
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

impl Completions {
    /// The candidate a keyboard accept lands on — [`Self::selected_idx`]
    /// clamped into `items`, or `None` for an empty popup.
    fn current_idx(&self) -> Option<usize> {
        if self.items.is_empty() {
            None
        } else {
            Some(self.selected_idx.min(self.items.len() - 1))
        }
    }

    /// Resolve a keystroke against the popup: Escape dismisses, Enter/Tab
    /// accept the highlighted candidate, and every
    /// [`NavKey`] moves the highlight through [`FlatNav`] with the box's
    /// own row count as the page size.
    fn handle_key(
        &self,
        key: &KeyEvent,
        layout: &CompletionsPrimitiveLayout,
    ) -> Handled<CompletionsEvent> {
        match key.key {
            Key::Named(NamedKey::Escape) => Handled::Event(CompletionsEvent::Dismissed),
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Tab) => match self.current_idx() {
                Some(idx) => Handled::Event(CompletionsEvent::Accepted { idx }),
                None => Handled::Ignored,
            },
            _ => {
                let Some(nav) = NavKey::from_key(key) else {
                    return Handled::Ignored;
                };
                // `NavKey::from_key` can't know this box's page size, so
                // it reports the physical Page keys as a 1-step move —
                // see its own doc. Re-magnify them here against the rows
                // actually on screen.
                let nav = match nav {
                    NavKey::PageUp(_) => NavKey::PageUp(layout.page()),
                    NavKey::PageDown(_) => NavKey::PageDown(layout.page()),
                    other => other,
                };
                let target =
                    FlatNav::mv(self.items.len(), self.current_idx(), nav, false, |_| true);
                match target {
                    Some(idx) if Some(idx) != self.current_idx() => {
                        Handled::Event(CompletionsEvent::SelectionChanged { idx })
                    }
                    // Already at the edge the key moves toward: the popup
                    // owns the keystroke (an editor must not also move its
                    // caret) but has nothing to report.
                    Some(_) => Handled::Consumed,
                    None => Handled::Ignored,
                }
            }
        }
    }
}

impl Interactive for Completions {
    /// Nothing: the highlight lives on [`Completions::selected_idx`] and
    /// the scroll offset is derived in
    /// [`Primitive::layout`], so this
    /// widget has no interaction state the descriptor doesn't already
    /// carry.
    type State = ();
    type Action = CompletionsEvent;

    /// One focus stop as a whole while the popup is live — it navigates
    /// internally through [`Self::handle`]'s own key dispatch rather than
    /// declaring a stop per row. [`FocusSpec::None`] for an empty popup,
    /// and for a suppressed one ([`Completions::has_focus`] `false`),
    /// which the host renders but does not route keys to.
    fn focus(&self) -> FocusSpec {
        if self.items.is_empty() || !self.has_focus {
            FocusSpec::None
        } else {
            FocusSpec::Stop
        }
    }

    fn handle(
        &self,
        _state: &mut (),
        event: &RoutedEvent,
        layout: &CompletionsPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<CompletionsEvent> {
        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                local,
                ..
            } => match layout.hit_test_local(local.x, local.y) {
                CompletionsHit::Item(idx) => {
                    cx.request_focus(None);
                    if Some(idx) == self.current_idx() {
                        Handled::Consumed
                    } else {
                        Handled::Event(CompletionsEvent::SelectionChanged { idx })
                    }
                }
                // Border padding inside the box: the popup swallows the
                // click rather than letting it fall through to the editor
                // behind it.
                CompletionsHit::Inert => Handled::Consumed,
                CompletionsHit::Empty => Handled::Event(CompletionsEvent::Dismissed),
            },
            RoutedEvent::Pointer {
                kind: PointerKind::DoubleClick(MouseButton::Left),
                local,
                ..
            } => match layout.hit_test_local(local.x, local.y) {
                CompletionsHit::Item(idx) => Handled::Event(CompletionsEvent::Accepted { idx }),
                _ => Handled::Ignored,
            },
            RoutedEvent::Key(key) => self.handle_key(key, layout),
            // `A11yAction` carries no row index through this generic seam,
            // so `Click` activates whichever candidate is highlighted —
            // the same one Enter accepts, matching the
            // assistive-technology default-action convention.
            RoutedEvent::Action(A11yAction::Click) => match self.current_idx() {
                Some(idx) => Handled::Event(CompletionsEvent::Accepted { idx }),
                None => Handled::Ignored,
            },
            _ => Handled::Ignored,
        }
    }

    fn a11y_actions(&self, _state: &()) -> Vec<A11yAction> {
        if self.items.is_empty() {
            Vec::new()
        } else {
            vec![A11yAction::Click]
        }
    }

    /// One sub-part per visible candidate, addressed by its index into
    /// [`Completions::items`], in [`Self::handle`]'s own local space.
    fn sub_regions(&self, layout: &CompletionsPrimitiveLayout) -> Vec<(Rect, SubTarget)> {
        layout
            .layout
            .visible_items
            .iter()
            .map(|vis| {
                (
                    Rect::new(
                        vis.bounds.x - layout.area.x,
                        vis.bounds.y - layout.area.y,
                        vis.bounds.width,
                        vis.bounds.height,
                    ),
                    SubTarget(vis.item_idx as u64),
                )
            })
            .collect()
    }
}

/// The cell-grid painter for [`Completions`], written against
/// [`crate::CellSurface`].
///
/// A thin vertical list: side `│` borders frame every row — the row
/// geometry has no top/bottom rows to spend on chrome — and the selected
/// row's background is tinted. Each row paints, left to right: the kind
/// icon ([`CompletionItem::resolved_icon`]'s `fallback`, since TUI has no
/// Nerd Font glyph distinct from it to offer), every span of `label`, and
/// — right-aligned, pinned to the row's own width — `detail`, muted. The
/// same content [`native_surface_paint::paint`] puts on a pixel surface,
/// degraded onto the cell grid (see [`crate::paint_surface`]'s module
/// doc, "Why TUI stays out").
///
/// Colours: [`crate::Theme::completion_bg`] for the row background,
/// [`crate::Theme::completion_selected_bg`] for the selected row,
/// [`crate::Theme::completion_fg`] for item text,
/// [`crate::Theme::muted_fg`] for `detail`, and
/// [`crate::Theme::completion_border`] for the side borders. A box
/// narrower than three columns (two borders plus one content column)
/// paints nothing.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{Completions, CompletionsLayout};
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::text_util::display_width;
    use crate::theme::Theme;
    use crate::types::{Color, Decoration, StyledText};

    fn style(fg: Color, bg: Color) -> CellStyle {
        CellStyle {
            fg: Some(fg),
            bg: Some(bg),
            ..CellStyle::default()
        }
    }

    /// Paint `text`'s spans at `(x + start_col, y)`, clipped to
    /// `max_cols` display columns, and return the column past the last
    /// glyph written. A span with no `fg` of its own takes `default_fg`,
    /// or `dim_fg` when `decoration` is [`Decoration::Muted`] — the one
    /// colour rule the cell-grid row content shares with every other
    /// styled-text row in this crate.
    #[allow(clippy::too_many_arguments)]
    fn put_styled(
        surface: &mut dyn CellSurface,
        x: u16,
        y: u16,
        start_col: u16,
        text: &StyledText,
        default_fg: Color,
        bg: Color,
        decoration: Decoration,
        dim_fg: Color,
        max_cols: u16,
    ) -> u16 {
        let mut col = start_col;
        for span in &text.spans {
            if col >= max_cols {
                break;
            }
            let fg = span
                .fg
                .unwrap_or(if matches!(decoration, Decoration::Muted) {
                    dim_fg
                } else {
                    default_fg
                });
            let mut span_style = style(fg, span.bg.unwrap_or(bg));
            span_style.bold = span.bold;
            span_style.italic = span.italic;
            col += surface.put_str_clipped(x + col, y, &span.text, max_cols - col, span_style);
        }
        col
    }

    /// Paint `completions` at its resolved `layout` onto `surface`.
    pub(crate) fn paint(
        completions: &Completions,
        layout: &CompletionsLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
    ) {
        let bg = theme.completion_bg;
        let sel_bg = theme.completion_selected_bg;
        let fg = theme.completion_fg;
        let dim_fg = theme.muted_fg;
        let border = theme.completion_border;

        let x = layout.bounds.x.round() as u16;
        let y = layout.bounds.y.round() as u16;
        let w = layout.bounds.width.round() as u16;
        if w < 3 {
            return;
        }

        for vis in &layout.visible_items {
            let Some(item) = completions.items.get(vis.item_idx) else {
                continue;
            };
            let row_y = y + (vis.bounds.y - layout.bounds.y).round() as u16;
            let is_selected = vis.item_idx == completions.selected_idx;
            let row_bg = if is_selected { sel_bg } else { bg };

            surface.fill(
                crate::Rect::new(x as f32, row_y as f32, w as f32, 1.0),
                style(fg, row_bg),
            );
            surface.put_char(x, row_y, '│', style(border, bg));
            surface.put_char(x + w - 1, row_y, '│', style(border, bg));

            // Row content lives between the two side borders.
            let content_x = x + 1;
            let content_w = w - 2;

            // Kind icon + one trailing pad column. Every completion
            // paints an icon — `resolved_icon` always has one (`item.icon`
            // or the kind's own default).
            let icon = item.resolved_icon();
            let mut col = surface.put_str_clipped(
                content_x,
                row_y,
                &icon.fallback,
                content_w,
                style(fg, row_bg),
            );
            if !icon.fallback.is_empty() && col < content_w {
                col += 1;
            }

            let label_end = put_styled(
                surface,
                content_x,
                row_y,
                col,
                &item.label,
                fg,
                row_bg,
                Decoration::Normal,
                dim_fg,
                content_w,
            );

            // Detail text — right-aligned within the row, muted, painted
            // only if it would not collide with the label just drawn.
            if let Some(ref detail) = item.detail {
                let detail_w: usize = detail.spans.iter().map(|s| display_width(&s.text)).sum();
                let start = (content_w as usize).saturating_sub(detail_w + 1) as u16;
                if start > label_end {
                    put_styled(
                        surface,
                        content_x,
                        row_y,
                        start,
                        detail,
                        dim_fg,
                        row_bg,
                        Decoration::Muted,
                        dim_fg,
                        content_w,
                    );
                }
            }
        }
    }
}

/// Shared pixel-surface paint for [`Completions`] — the single place a
/// GUI backend's popup paint logic lives, instead of each pixel backend
/// (GTK/Cairo, macOS/CoreGraphics, Windows/Direct2D) carrying its own
/// copy that can silently drift from the other two. TUI is not here —
/// see `crate::paint_surface`'s module doc ("Why TUI stays out") — its
/// painter lives in `crate::tui::completions` and degrades the same
/// content (kind icon, every label span, detail) onto a cell grid
/// instead of this trait's pixel-`Rect` verbs.
///
/// Every pixel backend reaches it the same two ways: through
/// [`Primitive::paint`](crate::primitive::Primitive::paint), and through
/// its own `gtk`/`macos`/`win::completions::draw_completions` wrapper,
/// which is a thin adapter from that backend's drawing context onto a
/// [`PaintSurface`] plus this call.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) mod native_surface_paint {
    use super::{Completions, CompletionsLayout};
    use crate::paint_surface::PaintSurface;
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::Rect;

    /// Paint a [`Completions`] popup at its resolved [`CompletionsLayout`]:
    /// background + border, then each visible row's selection fill, kind
    /// icon ([`super::CompletionItem::resolved_icon`]), every label span,
    /// and right-aligned `detail`.
    ///
    /// - `nerd_fonts_enabled` selects `Icon::glyph` vs. `Icon::fallback`
    ///   for the kind icon, same as every other icon-painting rasteriser
    ///   in this crate.
    /// - `style` supplies [`Style::row_padding_x`], the inset a row's
    ///   icon starts at.
    pub(crate) fn paint(
        completions: &Completions,
        layout: &CompletionsLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
        nerd_fonts_enabled: bool,
    ) {
        let bounds = layout.bounds;
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }

        surface.surface_fill_rect(bounds, theme.completion_bg);
        surface.surface_stroke_rect(bounds, theme.completion_border, 1.0);

        for vis in &layout.visible_items {
            let Some(item) = completions.items.get(vis.item_idx) else {
                continue;
            };
            let row = vis.bounds;
            let is_selected = vis.item_idx == completions.selected_idx;
            let row_bg = if is_selected {
                theme.completion_selected_bg
            } else {
                theme.completion_bg
            };
            if is_selected {
                surface.surface_fill_rect(row, row_bg);
            }

            surface.surface_push_clip(row);
            let mut cursor_x = row.x + style.row_padding_x;

            let icon = item.resolved_icon();
            let glyph = if nerd_fonts_enabled {
                icon.glyph.as_str()
            } else {
                icon.fallback.as_str()
            };
            if !glyph.is_empty() {
                let (iw, ih) = surface.surface_measure_text(glyph);
                surface.surface_draw_text_run(
                    Rect::new(cursor_x, row.y + (row.height - ih) / 2.0, iw, ih),
                    glyph,
                    theme.completion_fg,
                );
                cursor_x += iw + 6.0;
            }

            let detail_info = item.detail.as_ref().map(|detail| {
                let detail_text: String = detail.spans.iter().map(|s| s.text.as_str()).collect();
                let (dw, _) = surface.surface_measure_text(&detail_text);
                (detail_text, dw)
            });
            let detail_reserve = detail_info.as_ref().map(|(_, dw)| *dw + 8.0).unwrap_or(0.0);
            let text_right_limit = row.x + row.width - detail_reserve - 4.0;

            // A span that overflows `text_right_limit` is ellipsis-
            // truncated via the shared `fit_text_prefix` rather than
            // hard-clipped mid-glyph — same treatment as `ListView`'s
            // and `TreeView`'s rasterisers.
            for span in &item.label.spans {
                let remaining = text_right_limit - cursor_x;
                if remaining <= 0.0 {
                    break;
                }
                let span_fg = span.fg.unwrap_or(theme.completion_fg);
                let (sw, sh) = surface.surface_measure_text_styled(&span.text, span.bold);
                let (paint_text, paint_w, truncated) = if sw <= remaining {
                    (span.text.clone(), sw, false)
                } else {
                    let (t, w) = crate::paint_surface::fit_text_prefix(
                        surface, &span.text, span.bold, remaining,
                    );
                    (t, w, true)
                };
                if paint_w <= 0.0 {
                    break;
                }
                if let Some(sbg) = span.bg {
                    surface.surface_fill_rect(Rect::new(cursor_x, row.y, paint_w, row.height), sbg);
                }
                surface.surface_draw_text_run_styled(
                    Rect::new(cursor_x, row.y + (row.height - sh) / 2.0, paint_w, sh),
                    &paint_text,
                    span_fg,
                    span.bold,
                    false,
                    false,
                    1.0,
                );
                cursor_x += paint_w;
                if truncated {
                    break;
                }
            }

            surface.surface_pop_clip();

            // Detail text is pinned to the row's right edge, not the
            // h-scroll-shifted label — there is no h-scroll here, but
            // this keeps the same "measure outside the label clip"
            // shape `ListView`'s rasteriser uses.
            if let Some((detail_text, dw)) = detail_info {
                let dx = row.x + row.width - dw - 4.0;
                if dx > cursor_x {
                    let (_, dh) = surface.surface_measure_text(&detail_text);
                    surface.surface_draw_text_run(
                        Rect::new(dx, row.y + (row.height - dh) / 2.0, dw, dh),
                        &detail_text,
                        theme.muted_fg,
                    );
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::completions::{
            CompletionItem, CompletionItemMeasure, CompletionKind,
        };
        use crate::types::{Color, StyledSpan, StyledText, WidgetId};
        use crate::Image;

        /// Records every fill + stroke + styled text-run call — mirrors
        /// `primitives::list`'s `RecordingSurface` test double, scoped to
        /// the verbs [`paint`] uses, so this test runs on any host
        /// without Cairo/Core Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            strokes: Vec<(Rect, Color, f32)>,
            texts: Vec<(Rect, String, Color)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(400.0, 200.0, 1.0)
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
                rect: Rect,
                _radius: f32,
                color: Color,
                stroke_width: f32,
            ) {
                self.strokes.push((rect, color, stroke_width));
            }
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.texts.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(&mut self, _from: Point, _to: Point, _color: Color, _sw: f32) {}
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        use crate::event::Point;

        fn completions_with(items: Vec<CompletionItem>) -> Completions {
            Completions {
                id: WidgetId::new("c"),
                items,
                selected_idx: 1,
                scroll_offset: 0,
                has_focus: true,
            }
        }

        fn layout_for(c: &Completions) -> CompletionsLayout {
            let viewport = Rect::new(0.0, 0.0, 400.0, 200.0);
            c.layout(10.0, 10.0, 16.0, viewport, 300.0, 100.0, |_| {
                CompletionItemMeasure::new(20.0)
            })
        }

        #[test]
        fn paints_kind_icon_ahead_of_the_label() {
            let items = vec![CompletionItem {
                label: StyledText::plain("map"),
                detail: None,
                documentation: None,
                kind: CompletionKind::Function,
                icon: None,
            }];
            let c = completions_with(items);
            let l = layout_for(&c);
            let mut surface = RecordingSurface::default();
            paint(
                &c,
                &l,
                &mut surface,
                &Theme::default(),
                &Style::default(),
                true,
            );
            let icon = CompletionKind::Function.default_icon();
            assert!(
                surface.texts.iter().any(|(_, text, _)| text == &icon.glyph),
                "kind icon {:?} not painted: {:?}",
                icon.glyph,
                surface.texts
            );
        }

        #[test]
        fn paints_every_label_span() {
            let items = vec![CompletionItem {
                label: StyledText {
                    spans: vec![StyledSpan::plain("fn "), StyledSpan::plain("main")],
                },
                detail: None,
                documentation: None,
                kind: CompletionKind::Text,
                icon: None,
            }];
            let c = completions_with(items);
            let l = layout_for(&c);
            let mut surface = RecordingSurface::default();
            paint(
                &c,
                &l,
                &mut surface,
                &Theme::default(),
                &Style::default(),
                false,
            );
            assert!(surface.texts.iter().any(|(_, text, _)| text == "fn "));
            assert!(surface.texts.iter().any(|(_, text, _)| text == "main"));
        }

        #[test]
        fn paints_detail_text() {
            let items = vec![CompletionItem {
                label: StyledText::plain("map"),
                detail: Some(StyledText::plain("fn(T) -> U")),
                documentation: None,
                kind: CompletionKind::Method,
                icon: None,
            }];
            let c = completions_with(items);
            let l = layout_for(&c);
            let mut surface = RecordingSurface::default();
            paint(
                &c,
                &l,
                &mut surface,
                &Theme::default(),
                &Style::default(),
                false,
            );
            assert!(surface
                .texts
                .iter()
                .any(|(_, text, _)| text == "fn(T) -> U"));
        }

        #[test]
        fn fills_the_selected_row_with_completion_selected_bg() {
            let items = vec![
                CompletionItem {
                    label: StyledText::plain("a"),
                    detail: None,
                    documentation: None,
                    kind: CompletionKind::Text,
                    icon: None,
                },
                CompletionItem {
                    label: StyledText::plain("b"),
                    detail: None,
                    documentation: None,
                    kind: CompletionKind::Text,
                    icon: None,
                },
            ];
            let c = completions_with(items); // selected_idx: 1
            let l = layout_for(&c);
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(&c, &l, &mut surface, &theme, &Style::default(), false);
            let selected_row = l.visible_items[1].bounds;
            assert!(surface.fills.iter().any(
                |(rect, color)| *rect == selected_row && *color == theme.completion_selected_bg
            ));
            // The unselected row must not get that fill.
            let other_row = l.visible_items[0].bounds;
            assert!(!surface
                .fills
                .iter()
                .any(|(rect, color)| *rect == other_row && *color == theme.completion_selected_bg));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Completions primitive tests (D6) ──────────────────────────────

    fn make_completion(label: &str, kind: CompletionKind) -> CompletionItem {
        CompletionItem {
            label: StyledText::plain(label),
            detail: None,
            documentation: None,
            kind,
            icon: None,
        }
    }

    #[test]
    fn completions_layout_below_cursor() {
        let c = Completions {
            id: WidgetId::new("c"),
            items: vec![
                make_completion("fn main", CompletionKind::Function),
                make_completion("fn map", CompletionKind::Function),
                make_completion("Vec", CompletionKind::Struct),
            ],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = c.layout(100.0, 50.0, 18.0, viewport, 300.0, 200.0, |_| {
            CompletionItemMeasure::new(20.0)
        });
        assert_eq!(layout.placement, CompletionsPlacement::Below);
        assert_eq!(layout.bounds.x, 100.0);
        // Popup y = cursor_y + line_height = 50 + 18 = 68
        assert_eq!(layout.bounds.y, 68.0);
        assert_eq!(layout.visible_items.len(), 3);
        assert_eq!(layout.visible_items[0].bounds.y, 68.0);
        // Click on 2nd item.
        match layout.hit_test(150.0, 90.0) {
            CompletionsHit::Item(idx) => assert_eq!(idx, 1),
            _ => panic!(),
        }
    }

    #[test]
    fn completions_layout_flips_above_when_below_overflows() {
        let c = Completions {
            id: WidgetId::new("c"),
            items: (0..5)
                .map(|i| make_completion(&format!("x{i}"), CompletionKind::Variable))
                .collect(),
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        };
        // Cursor near the bottom of a small viewport.
        let viewport = Rect::new(0.0, 0.0, 800.0, 150.0);
        // Cursor at y=120, line_height=18 → below_y = 138, need 100 px (5 × 20)
        // → bottom edge = 238, > 150 viewport. Flip above.
        let layout = c.layout(100.0, 120.0, 18.0, viewport, 300.0, 200.0, |_| {
            CompletionItemMeasure::new(20.0)
        });
        assert_eq!(layout.placement, CompletionsPlacement::Above);
        // Above: y = cursor - content_h = 120 - 100 = 20
        assert_eq!(layout.bounds.y, 20.0);
    }

    #[test]
    fn completions_layout_shifts_left_when_right_overflows() {
        let c = Completions {
            id: WidgetId::new("c"),
            items: vec![make_completion("item", CompletionKind::Text)],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        };
        let viewport = Rect::new(0.0, 0.0, 400.0, 600.0);
        // Cursor near right edge — popup_width 300, cursor_x 200 → right would be 500 > 400.
        let layout = c.layout(200.0, 50.0, 18.0, viewport, 300.0, 200.0, |_| {
            CompletionItemMeasure::new(20.0)
        });
        assert_eq!(layout.bounds.x, 100.0); // 400 - 300
    }

    #[test]
    fn completions_layout_scroll_offset_applies() {
        let c = Completions {
            id: WidgetId::new("c"),
            items: (0..10)
                .map(|i| make_completion(&format!("x{i}"), CompletionKind::Variable))
                .collect(),
            selected_idx: 0,
            scroll_offset: 5,
            has_focus: true,
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = c.layout(0.0, 0.0, 18.0, viewport, 200.0, 200.0, |_| {
            CompletionItemMeasure::new(20.0)
        });
        // Visible items start at index 5.
        assert_eq!(layout.visible_items[0].item_idx, 5);
    }

    #[test]
    fn completions_layout_pins_below_when_neither_side_fully_fits() {
        // 3 items * 20 = 60 desired height; viewport only 100 tall with
        // the cursor near its middle, so neither the `below_y`/`above_y`
        // candidate fits without clamping.
        //
        // When neither side fits, the popup pins to the preferred side
        // (`Below`) and clamps — landing pinned to the viewport's bottom
        // edge (y=40, i.e. `bounds.y + bounds.height == 100`), placing
        // the (unclipped) 60-tall popup fully inside the 100-tall
        // viewport.
        let c = Completions {
            id: WidgetId::new("c"),
            items: (0..3)
                .map(|i| make_completion(&format!("x{i}"), CompletionKind::Variable))
                .collect(),
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 100.0);
        let layout = c.layout(100.0, 55.0, 5.0, viewport, 200.0, 60.0, |_| {
            CompletionItemMeasure::new(20.0)
        });
        assert_eq!(layout.placement, CompletionsPlacement::Below);
        assert_eq!(layout.bounds.y, 40.0);
        assert_eq!(layout.bounds.height, 60.0);
    }

    #[test]
    fn completions_layout_clamps_x_to_viewport_left_edge() {
        // Cursor at x=0 with `popup_width` larger than it — `x` must
        // still clamp to the viewport's own left edge rather than go
        // negative.
        let c = Completions {
            id: WidgetId::new("c"),
            items: vec![make_completion("item", CompletionKind::Text)],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        };
        let viewport = Rect::new(10.0, 0.0, 400.0, 600.0);
        let layout = c.layout(0.0, 50.0, 18.0, viewport, 300.0, 200.0, |_| {
            CompletionItemMeasure::new(20.0)
        });
        assert_eq!(layout.bounds.x, 10.0);
    }

    #[test]
    fn completions_layout_flips_above_when_cursor_overflows_both_axes() {
        // Caret in the bottom-right corner of the viewport: overflows
        // horizontally (popup_width 40 past cursor_x 50 in an 80-wide
        // viewport) AND the `Below` candidate overflows vertically
        // (cursor near the bottom). The vertical flip to `Above` must
        // still happen — horizontal overflow is clamped independently
        // and must not suppress it.
        let c = Completions {
            id: WidgetId::new("c"),
            items: (0..10)
                .map(|i| make_completion(&format!("x{i}"), CompletionKind::Variable))
                .collect(),
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        };
        let viewport = Rect::new(0.0, 0.0, 80.0, 24.0);
        let layout = c.layout(50.0, 20.0, 1.0, viewport, 40.0, 24.0, |_| {
            CompletionItemMeasure::new(1.0)
        });
        assert_eq!(layout.placement, CompletionsPlacement::Above);
        // x clamps to the right edge: 80 - 40 = 40.
        assert_eq!(layout.bounds.x, 40.0);
        // Above: y = cursor_y - desired_height = 20 - 10 = 10.
        assert_eq!(layout.bounds.y, 10.0);
    }

    // ── Cell-path paint (`cell_paint::paint`) ───────────────────────────

    #[cfg(feature = "tui")]
    mod cells {
        use super::*;
        use crate::event::Rect as QRect;
        use crate::theme::Theme;
        use crate::tui::{draw_completions, ratatui_color};
        use crate::types::StyledSpan;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect as RRect;

        fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
            buf[(x, y)].symbol().chars().next().unwrap_or(' ')
        }

        fn cell_at(buf: &Buffer, x: u16, y: u16) -> &ratatui::buffer::Cell {
            &buf[(x, y)]
        }

        fn completions_with(items: Vec<CompletionItem>, selected_idx: usize) -> Completions {
            Completions {
                id: WidgetId::new("c"),
                items,
                selected_idx,
                scroll_offset: 0,
                has_focus: true,
            }
        }

        /// Anchors the cursor one line above the viewport's top edge, so
        /// `below_y = cursor_y + line_height` lands the popup's first row
        /// at buffer row `0` — the tests below index `buf` directly by
        /// row, so they need a fixed, known `bounds.y`.
        fn layout_for(c: &Completions, w: u16, h: u16) -> CompletionsLayout {
            let viewport = QRect::new(0.0, 0.0, w as f32, h as f32);
            c.layout(0.0, -1.0, 1.0, viewport, w as f32, h as f32, |_| {
                CompletionItemMeasure::new(1.0)
            })
        }

        #[test]
        fn paints_the_kind_icon_ahead_of_the_label() {
            let items = vec![CompletionItem {
                label: StyledText::plain("map"),
                detail: None,
                documentation: None,
                kind: CompletionKind::Function,
                icon: None,
            }];
            let c = completions_with(items, 0);
            let l = layout_for(&c, 20, 3);
            let mut buf = Buffer::empty(RRect::new(0, 0, 20, 3));
            draw_completions(&mut buf, &c, &l, &Theme::default());

            let icon = CompletionKind::Function.default_icon();
            let painted: String = icon
                .fallback
                .chars()
                .enumerate()
                .map(|(i, _)| cell_char(&buf, 1 + i as u16, 0))
                .collect();
            assert_eq!(painted, icon.fallback);
        }

        #[test]
        fn paints_every_label_span() {
            let items = vec![CompletionItem {
                label: StyledText {
                    spans: vec![StyledSpan::plain("fn "), StyledSpan::plain("main")],
                },
                detail: None,
                documentation: None,
                kind: CompletionKind::Text,
                icon: None,
            }];
            let c = completions_with(items, 0);
            let l = layout_for(&c, 20, 3);
            let mut buf = Buffer::empty(RRect::new(0, 0, 20, 3));
            draw_completions(&mut buf, &c, &l, &Theme::default());

            // Icon ("tx") + pad occupies cols 1..3; the label starts at col 4.
            let label_start = 1 + CompletionKind::Text.default_icon().fallback.chars().count() + 1;
            let painted: String = "fn main"
                .chars()
                .enumerate()
                .map(|(i, _)| cell_char(&buf, label_start as u16 + i as u16, 0))
                .collect();
            assert_eq!(painted, "fn main");
        }

        #[test]
        fn paints_detail_right_aligned_and_muted() {
            let items = vec![CompletionItem {
                label: StyledText::plain("map"),
                detail: Some(StyledText::plain("fn(T) -> U")),
                documentation: None,
                kind: CompletionKind::Method,
                icon: None,
            }];
            let c = completions_with(items, 0);
            let w = 24u16;
            let l = layout_for(&c, w, 3);
            let mut buf = Buffer::empty(RRect::new(0, 0, w, 3));
            let theme = Theme::default();
            draw_completions(&mut buf, &c, &l, &theme);

            let detail = "fn(T) -> U";
            // Detail is right-aligned within the row, one cell before the
            // right border.
            let start = (w - 2) as usize - detail.chars().count() - 1 + 1; // content_x offset
            let painted: String = detail
                .chars()
                .enumerate()
                .map(|(i, _)| cell_char(&buf, start as u16 + i as u16, 0))
                .collect();
            assert_eq!(painted, detail);
            let fg = cell_at(&buf, start as u16, 0).fg;
            assert_eq!(fg, ratatui_color(theme.muted_fg));
        }

        #[test]
        fn selected_row_gets_completion_selected_bg() {
            let items = vec![
                CompletionItem {
                    label: StyledText::plain("a"),
                    detail: None,
                    documentation: None,
                    kind: CompletionKind::Text,
                    icon: None,
                },
                CompletionItem {
                    label: StyledText::plain("b"),
                    detail: None,
                    documentation: None,
                    kind: CompletionKind::Text,
                    icon: None,
                },
            ];
            let c = completions_with(items, 1);
            let l = layout_for(&c, 20, 3);
            let mut buf = Buffer::empty(RRect::new(0, 0, 20, 3));
            let theme = Theme::default();
            draw_completions(&mut buf, &c, &l, &theme);

            assert_eq!(
                cell_at(&buf, 2, 1).bg,
                ratatui_color(theme.completion_selected_bg)
            );
            assert_eq!(cell_at(&buf, 2, 0).bg, ratatui_color(theme.completion_bg));
        }

        #[test]
        fn side_borders_frame_every_row() {
            let items = vec![CompletionItem {
                label: StyledText::plain("map"),
                detail: None,
                documentation: None,
                kind: CompletionKind::Text,
                icon: None,
            }];
            let c = completions_with(items, 0);
            let l = layout_for(&c, 20, 3);
            let mut buf = Buffer::empty(RRect::new(0, 0, 20, 3));
            draw_completions(&mut buf, &c, &l, &Theme::default());
            assert_eq!(cell_char(&buf, 0, 0), '│');
            assert_eq!(cell_char(&buf, 19, 0), '│');
        }

        /// A box narrower than the two borders plus one content column
        /// has nowhere to paint, so it paints nothing at all rather than
        /// a stray border column.
        #[test]
        fn a_box_under_three_columns_wide_paints_nothing() {
            let items = vec![CompletionItem {
                label: StyledText::plain("map"),
                detail: None,
                documentation: None,
                kind: CompletionKind::Text,
                icon: None,
            }];
            let c = completions_with(items, 0);
            let l = layout_for(&c, 2, 3);
            let mut buf = Buffer::empty(RRect::new(0, 0, 10, 3));
            draw_completions(&mut buf, &c, &l, &Theme::default());
            assert_eq!(cell_char(&buf, 0, 0), ' ');
            assert_eq!(cell_char(&buf, 1, 0), ' ');
        }
    }

    // ── Widget contract: `Primitive` + `Interactive` ────────────────────

    mod contract {
        use super::*;
        use crate::interactive::{EventHost, FocusSpec, PointerKind, SubTarget};
        use crate::primitive::{LayoutCtx, Primitive, Size, Units};
        use crate::types::Modifiers;
        use crate::{Point, PointerShape};

        /// Minimal [`EventHost`] mock: records every call so a test can
        /// assert on what [`Interactive::handle`] asked for — the same
        /// shape `interactive`'s own module tests use.
        #[derive(Debug, Default)]
        struct MockHost {
            focus_requests: Vec<Option<SubTarget>>,
            emitted: Vec<crate::UiEvent>,
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
            fn emit(&mut self, event: crate::UiEvent) {
                self.emitted.push(event);
            }
            fn units(&self) -> Units {
                Units::Cells
            }
        }

        fn popup(items: usize, selected_idx: usize, scroll_offset: usize) -> Completions {
            Completions {
                id: WidgetId::new("completions:test"),
                items: (0..items)
                    .map(|i| make_completion(&format!("cand{i}"), CompletionKind::Function))
                    .collect(),
                selected_idx,
                scroll_offset,
                has_focus: true,
            }
        }

        fn ctx() -> LayoutCtx<'static> {
            LayoutCtx::new(1.0, 1.0, false)
        }

        fn layout_for(c: &Completions, rows: f32) -> CompletionsPrimitiveLayout {
            Primitive::layout(c, &ctx(), Rect::new(4.0, 2.0, 20.0, rows))
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

        fn double_click_at(x: f32, y: f32) -> RoutedEvent {
            RoutedEvent::Pointer {
                kind: PointerKind::DoubleClick(MouseButton::Left),
                sub: None,
                local: Point::new(x, y),
                mods: Modifiers::default(),
            }
        }

        fn handle(
            c: &Completions,
            event: &RoutedEvent,
            layout: &CompletionsPrimitiveLayout,
        ) -> (Handled<CompletionsEvent>, MockHost) {
            let mut host = MockHost::default();
            let mut cx = EventCx::new(c.id, &mut host);
            let out = Interactive::handle(c, &mut (), event, layout, &mut cx);
            (out, host)
        }

        #[test]
        fn layout_places_every_row_inside_the_host_resolved_box() {
            let c = popup(3, 0, 0);
            let layout = layout_for(&c, 3.0);
            assert_eq!(layout.area, Rect::new(4.0, 2.0, 20.0, 3.0));
            assert_eq!(layout.layout.bounds, layout.area);
            assert_eq!(layout.layout.visible_items.len(), 3);
            assert_eq!(layout.layout.visible_items[0].bounds.y, 2.0);
            assert_eq!(layout.layout.visible_items[2].bounds.y, 4.0);
            assert_eq!(layout.layout.placement, CompletionsPlacement::Below);
        }

        /// A selection past the bottom of the box scrolls it back into
        /// view without the host tracking a second scroll field.
        #[test]
        fn layout_scrolls_the_selected_candidate_into_view() {
            let c = popup(10, 7, 0);
            let layout = layout_for(&c, 3.0);
            assert_eq!(layout.layout.resolved_scroll_offset, 5);
            let shown: Vec<usize> = layout
                .layout
                .visible_items
                .iter()
                .map(|v| v.item_idx)
                .collect();
            assert_eq!(shown, vec![5, 6, 7]);
        }

        #[test]
        fn layout_never_scrolls_past_the_last_full_window() {
            let c = popup(10, 0, 99);
            let layout = layout_for(&c, 3.0);
            assert_eq!(layout.layout.resolved_scroll_offset, 0);
            assert_eq!(layout.layout.visible_items[0].item_idx, 0);
        }

        #[test]
        fn measure_reports_one_cell_row_per_candidate() {
            let c = popup(4, 0, 0);
            let natural = Primitive::measure(&c, &ctx(), Size::new(80.0, 24.0));
            assert_eq!(natural.height, Some(4.0));
            // "fn" icon + pad + "cand0" + two border columns.
            assert_eq!(natural.width, Some(2.0 + 1.0 + 5.0 + 2.0));
        }

        #[test]
        fn hit_test_local_resolves_each_visible_row() {
            let c = popup(3, 0, 0);
            let layout = layout_for(&c, 3.0);
            assert_eq!(layout.hit_test_local(1.0, 0.5), CompletionsHit::Item(0));
            assert_eq!(layout.hit_test_local(1.0, 2.5), CompletionsHit::Item(2));
            assert_eq!(layout.hit_test_local(1.0, 9.0), CompletionsHit::Empty);
        }

        #[test]
        fn sub_regions_name_every_visible_candidate_in_local_space() {
            let c = popup(10, 7, 0);
            let layout = layout_for(&c, 3.0);
            let regions = Interactive::sub_regions(&c, &layout);
            assert_eq!(
                regions.iter().map(|(_, s)| s.0).collect::<Vec<_>>(),
                vec![5, 6, 7]
            );
            assert_eq!(regions[0].0, Rect::new(0.0, 0.0, 20.0, 1.0));
        }

        #[test]
        fn an_empty_or_suppressed_popup_is_not_a_focus_stop() {
            assert_eq!(Interactive::focus(&popup(0, 0, 0)), FocusSpec::None);
            let mut suppressed = popup(3, 0, 0);
            suppressed.has_focus = false;
            assert_eq!(Interactive::focus(&suppressed), FocusSpec::None);
            assert_eq!(Interactive::focus(&popup(3, 0, 0)), FocusSpec::Stop);
        }

        #[test]
        fn arrow_keys_move_the_highlight_by_one() {
            let c = popup(4, 1, 0);
            let layout = layout_for(&c, 4.0);
            let (down, _) = handle(&c, &key(NamedKey::Down), &layout);
            assert_eq!(
                down,
                Handled::Event(CompletionsEvent::SelectionChanged { idx: 2 })
            );
            let (up, _) = handle(&c, &key(NamedKey::Up), &layout);
            assert_eq!(
                up,
                Handled::Event(CompletionsEvent::SelectionChanged { idx: 0 })
            );
        }

        #[test]
        fn page_keys_move_by_the_rows_actually_on_screen() {
            let c = popup(10, 0, 0);
            let layout = layout_for(&c, 3.0);
            assert_eq!(layout.page(), 3);
            let (paged, _) = handle(&c, &key(NamedKey::PageDown), &layout);
            assert_eq!(
                paged,
                Handled::Event(CompletionsEvent::SelectionChanged { idx: 3 })
            );
        }

        #[test]
        fn home_and_end_jump_to_the_first_and_last_candidate() {
            let c = popup(10, 4, 0);
            let layout = layout_for(&c, 3.0);
            let (home, _) = handle(&c, &key(NamedKey::Home), &layout);
            assert_eq!(
                home,
                Handled::Event(CompletionsEvent::SelectionChanged { idx: 0 })
            );
            let (end, _) = handle(&c, &key(NamedKey::End), &layout);
            assert_eq!(
                end,
                Handled::Event(CompletionsEvent::SelectionChanged { idx: 9 })
            );
        }

        /// Navigation does not wrap: at the last candidate, Down keeps the
        /// keystroke (so an editor behind the popup never sees it) but
        /// reports nothing to apply.
        #[test]
        fn navigation_at_the_edge_is_consumed_without_an_event() {
            let c = popup(3, 2, 0);
            let layout = layout_for(&c, 3.0);
            let (out, _) = handle(&c, &key(NamedKey::Down), &layout);
            assert_eq!(out, Handled::Consumed);
        }

        #[test]
        fn enter_and_tab_accept_the_highlighted_candidate() {
            let c = popup(4, 2, 0);
            let layout = layout_for(&c, 4.0);
            for named in [NamedKey::Enter, NamedKey::Tab] {
                let (out, _) = handle(&c, &key(named), &layout);
                assert_eq!(out, Handled::Event(CompletionsEvent::Accepted { idx: 2 }));
            }
        }

        #[test]
        fn escape_dismisses() {
            let c = popup(4, 2, 0);
            let layout = layout_for(&c, 4.0);
            let (out, _) = handle(&c, &key(NamedKey::Escape), &layout);
            assert_eq!(out, Handled::Event(CompletionsEvent::Dismissed));
        }

        #[test]
        fn an_unrelated_key_is_ignored() {
            let c = popup(4, 2, 0);
            let layout = layout_for(&c, 4.0);
            let event = RoutedEvent::Key(KeyEvent {
                key: Key::Char('x'),
                modifiers: Modifiers::default(),
                repeat: false,
            });
            let (out, _) = handle(&c, &event, &layout);
            assert_eq!(out, Handled::Ignored);
        }

        #[test]
        fn a_click_selects_the_row_and_takes_focus() {
            let c = popup(4, 0, 0);
            let layout = layout_for(&c, 4.0);
            let (out, host) = handle(&c, &down_at(3.0, 2.5), &layout);
            assert_eq!(
                out,
                Handled::Event(CompletionsEvent::SelectionChanged { idx: 2 })
            );
            assert_eq!(host.focus_requests, vec![None]);
        }

        #[test]
        fn a_double_click_accepts_the_row_under_the_pointer() {
            let c = popup(4, 0, 0);
            let layout = layout_for(&c, 4.0);
            let (out, _) = handle(&c, &double_click_at(3.0, 1.5), &layout);
            assert_eq!(out, Handled::Event(CompletionsEvent::Accepted { idx: 1 }));
        }

        #[test]
        fn a_click_outside_the_box_dismisses() {
            let c = popup(4, 0, 0);
            let layout = layout_for(&c, 4.0);
            let (out, _) = handle(&c, &down_at(3.0, 40.0), &layout);
            assert_eq!(out, Handled::Event(CompletionsEvent::Dismissed));
        }

        #[test]
        fn an_a11y_click_accepts_the_highlighted_candidate() {
            let c = popup(4, 3, 0);
            let layout = layout_for(&c, 4.0);
            let (out, _) = handle(&c, &RoutedEvent::Action(A11yAction::Click), &layout);
            assert_eq!(out, Handled::Event(CompletionsEvent::Accepted { idx: 3 }));
            assert_eq!(Interactive::a11y_actions(&c, &()), vec![A11yAction::Click]);
            assert!(Interactive::a11y_actions(&popup(0, 0, 0), &()).is_empty());
        }

        #[test]
        fn actions_map_onto_the_crates_own_ui_events() {
            let id = WidgetId::new("completions:test");
            assert_eq!(
                CompletionsEvent::Accepted { idx: 2 }.into_ui_event(id),
                crate::UiEvent::CompletionItemActivated(id, 2)
            );
            assert_eq!(
                CompletionsEvent::SelectionChanged { idx: 1 }.into_ui_event(id),
                crate::UiEvent::List(id, crate::ListViewEvent::SelectionChanged { idx: 1 })
            );
            assert!(matches!(
                CompletionsEvent::Dismissed.into_ui_event(id),
                crate::UiEvent::List(_, crate::ListViewEvent::KeyPressed { .. })
            ));
        }
    }
}
