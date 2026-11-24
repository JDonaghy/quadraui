//! [`Toolbar`]'s widget contract: [`Primitive`] (one layout, one pixel
//! paint, one cell paint) plus [`Interactive`] (hover, press, click and
//! keyboard navigation), so a toolbar reaches every backend through
//! [`crate::backend::DrawsPrimitives::draw_primitive`]/`draw_dyn` and
//! sits in a [`crate::frame_tree::FrameTree`] as a
//! [`crate::frame::Surface::Widget`] without a `Backend` method pair of
//! its own.
//!
//! ## One layout, overflow included
//!
//! [`Primitive::layout`] branches on [`crate::primitive::Units`]: a cell
//! grid resolves through [`super::cell_layout`] (the exact resolver the
//! TUI rasteriser paints from), a pixel surface through
//! [`super::measure_button`] fed by the calling backend's own
//! [`crate::Backend::measure_text`]. Neither arm takes a per-call
//! options argument: overflow, orientation, the trailing-group clip
//! anchor and the vertical text alignment are all descriptor fields
//! ([`Toolbar::layout_options`], [`Toolbar::overflow_priorities`],
//! [`Toolbar::valign`]), so one `layout` call covers what the legacy
//! `_with_options`/`_with_overflow` entry-point family covers between
//! them. Both produce the same
//! [`ToolbarLayout`], in the widget's **own local space** (origin at the
//! painted rect) — the space [`Interactive::sub_regions`] and
//! [`crate::interactive::RoutedEvent::Pointer`]'s `local` both use, so
//! hit-testing needs no translation. [`Primitive::paint`] translates it
//! back by [`ToolbarPrimitiveLayout::area`]'s origin for the pixel
//! painter, which paints in surface-absolute coordinates.
//!
//! ## Interaction state
//!
//! [`ToolbarState`] is the live counterpart of the `hovered_id` /
//! `pressed_id` pair every `draw_toolbar*` entry point takes from its
//! host, and of [`Toolbar::focused_index`] for the keyboard cursor. It
//! implements [`crate::primitive::PaintState`], so the index half of
//! that state reaches a painter through the shared trait rather than a
//! per-widget sidecar.

use crate::a11y::A11yAction;
use crate::behaviour::nav::{FlatNav, NavKey};
use crate::event::{Key, MouseButton, NamedKey, Rect};
use crate::interactive::{
    EventCx, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget, WidgetAction,
};
use crate::primitive::{LayoutCtx, PaintState, Primitive, Size, Units};
use crate::types::WidgetId;
use crate::Handled;

use super::{
    Toolbar, ToolbarButton, ToolbarEvent, ToolbarHit, ToolbarItemKind, ToolbarItemMeasure,
    ToolbarLayout, ToolbarVAlign, MENU_BUTTON_CARET_PX,
};

/// Cell-unit width of a [`ToolbarButton::MenuButton`]'s trailing `▾`
/// affordance — the two cells `super::cell_item_width`'s own
/// `MenuButton` arm reserves (`" ▾"`), the cell-grid counterpart of
/// [`MENU_BUTTON_CARET_PX`].
// Read by the cell-grid `Primitive::layout` branch, so a build with no
// terminal backend compiled in has no caller for it.
#[cfg_attr(not(feature = "tui"), allow(dead_code))]
const MENU_BUTTON_CARET_CELLS: f32 = 2.0;

/// [`Primitive::Layout`] for [`Toolbar`]: the resolved
/// [`ToolbarLayout`], the area it was resolved for, and the two
/// unit-dependent facts a hit-test or a paint needs afterwards.
///
/// `#[non_exhaustive]`: a future field add here is additive, the same
/// convention [`ToolbarLayout`] itself follows.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct ToolbarPrimitiveLayout {
    /// Absolute area [`Primitive::layout`] was called with.
    pub area: Rect,
    /// The bar-relative layout itself — every `bounds` in it has its
    /// origin at `area`'s own origin, not at the surface origin.
    pub layout: ToolbarLayout,
    /// Where button/label text paints within a slot taller than one text
    /// row — the resolving descriptor's own [`Toolbar::valign`], carried
    /// here so [`Primitive::paint`]/[`Primitive::paint_cells`] need no
    /// options argument of their own.
    pub valign: ToolbarVAlign,
    /// Main-axis extent of a [`ToolbarButton::MenuButton`]'s trailing
    /// `▾` affordance in the unit this layout was resolved in —
    /// [`MENU_BUTTON_CARET_PX`] on a pixel surface,
    /// [`MENU_BUTTON_CARET_CELLS`] on a cell grid. Carving the dropdown
    /// arrow's own sub-region out of a menu button's slot needs it, and
    /// the resolved layout is otherwise unit-agnostic.
    pub caret_extent: f32,
}

impl ToolbarPrimitiveLayout {
    /// Which part of the toolbar sits under widget-local `(x, y)` — the
    /// descriptor's own [`ToolbarLayout::hit_test`], named here so a
    /// caller holding this layout doesn't have to reach through the
    /// `layout` field for it.
    pub fn hit_test(&self, x: f32, y: f32) -> ToolbarHit {
        self.layout.hit_test(x, y)
    }
}

// ── Primitive ───────────────────────────────────────────────────────

/// Translate every rect in `layout` by `(dx, dy)` — the bar-relative
/// layout [`Primitive::layout`] resolves, moved into the
/// surface-absolute space [`super::native_surface_paint::paint`] paints
/// in.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
fn translate_toolbar_layout(mut layout: ToolbarLayout, dx: f32, dy: f32) -> ToolbarLayout {
    let shift = |r: &mut Rect| {
        r.x += dx;
        r.y += dy;
    };
    shift(&mut layout.bar_bounds);
    for vis in &mut layout.visible_items {
        shift(&mut vis.bounds);
        for child in &mut vis.group_children {
            shift(&mut child.bounds);
        }
    }
    layout
}

/// `rect`'s size as a zero-origin cell area, through the same
/// [`crate::tui::backend::q_rect_to_ratatui`] rounding
/// [`Primitive::paint_cells`] converts the absolute rect with — so the
/// layout a hit test reads and the cells a paint writes can't disagree
/// about the bar's width by a rounding step on a fractional rect.
#[cfg(feature = "tui")]
fn cell_area(rect: Rect) -> ratatui::layout::Rect {
    let absolute = crate::tui::backend::q_rect_to_ratatui(rect);
    ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: absolute.width,
        height: absolute.height,
    }
}

impl Primitive for Toolbar {
    type Layout = ToolbarPrimitiveLayout;
    type Event = ToolbarEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// Round-trips through [`crate::frame::OwnedContent::Toolbar`] — the
    /// same owned snapshot [`crate::frame::Surface::Toolbar`] captures
    /// into. The replay paints through
    /// [`crate::Backend::draw_toolbar_interactive`], which reaches this
    /// module's own painter with the same resting hover/press state
    /// [`Self::paint_cells`] passes it, and resolves the bar's geometry
    /// from the method's own fixed `Clip`/`Horizontal`/`Top` arguments
    /// rather than from the descriptor's [`Toolbar::layout_options`] /
    /// [`Toolbar::valign`] — so a replay is byte-identical for a
    /// default-options descriptor, and a wrapping, vertical or
    /// bottom-aligned one replays as the clipped horizontal bar that
    /// method paints, until `Surface::Toolbar` itself is retired in
    /// favour of [`crate::frame::Surface::Widget`].
    ///
    /// The replayed bar reports
    /// [`crate::frame::FrameZone::Toolbar`] rather than
    /// [`crate::frame::FrameZone::Widget`], since that mapping belongs
    /// to `OwnedContent` itself.
    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Toolbar(self.clone())
    }

    /// The bar's own intrinsic main-axis extent: the summed width of
    /// every item, clamped to `avail`, with one text row on the cross
    /// axis. A container that stretches its children ignores this; one
    /// that sizes a toolbar to its content gets a strip exactly as wide
    /// as the buttons it holds.
    fn measure(&self, ctx: &LayoutCtx, avail: Size) -> crate::NaturalSize {
        // Both arms are unit-agnostic code in the widget module: the
        // cell measurer is `cell_item_width` (which needs no terminal
        // backend compiled in), the pixel one the `measure_button`
        // formula every GUI wrapper shares.
        let total: f32 = match ctx.units() {
            Units::Cells => self.buttons.iter().map(super::cell_item_width).sum(),
            Units::Pixels => {
                let measure = crate::primitives::layout_metrics::LayoutCtxMeasure {
                    ctx,
                    role: crate::FontRole::Chrome,
                };
                self.buttons
                    .iter()
                    .map(|btn| super::measure_button(&measure, btn))
                    .sum()
            }
        };
        let row = match ctx.units() {
            Units::Cells => 1.0,
            Units::Pixels => ctx.line_height,
        };
        crate::NaturalSize::new(Some(total.min(avail.width)), Some(row))
    }

    /// Resolve the bar-relative layout for `rect`, in whichever unit the
    /// caller measures in — see this module's own doc.
    ///
    /// Overflow, orientation and the trailing-group clip anchor all come
    /// off the descriptor ([`Toolbar::layout_options`] plus
    /// [`Toolbar::overflow_priorities`]), so this is the only layout a
    /// bar needs: a [`super::ToolbarOverflow::Wrap`] descriptor wraps
    /// here, a [`super::ToolbarOverflow::Menu`] one grows the trailing
    /// `»` affordance, and a [`super::ToolbarOrientation::Vertical`] one
    /// stacks — with no per-call options argument and no second entry
    /// point. A default descriptor resolves byte-identically to the
    /// plain `Clip`/`Horizontal` path, which both overflow resolvers
    /// delegate to for [`super::ToolbarOverflow::Clip`].
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> Self::Layout {
        let options = self.layout_options();
        // `ctx.units()` is a reliable *runtime* discriminator between a
        // TUI caller and a pixel-backend one even when both features are
        // compiled into the same binary — see `LayoutCtx::units`'s own
        // doc.
        #[cfg(feature = "tui")]
        {
            if matches!(ctx.units(), Units::Cells) {
                // The same resolver the cell rasteriser paints from
                // (group degrade tiers included, wrapped rows past the
                // bar's own height demoted to un-clickable), fed a
                // zero-origin area so the result is already
                // bar-relative.
                let area = cell_area(rect);
                return ToolbarPrimitiveLayout {
                    area: rect,
                    layout: super::cell_layout::tui_toolbar_layout_with_overflow(
                        self,
                        area,
                        options,
                        &self.overflow_priorities,
                    ),
                    valign: self.valign,
                    caret_extent: MENU_BUTTON_CARET_CELLS,
                };
            }
        }
        // Button labels, inline labels and key hints are chrome —
        // painted in the backend's UI font by `native_surface_paint`
        // — so this measures them through the calling backend's own
        // `Backend::measure_text` for `FontRole::Chrome`, the exact
        // formula `measure_button` shares with every pixel wrapper.
        // Without a measurer in reach (`LayoutCtx::new`) it degrades to
        // the `ctx.char_width` approximation.
        let measure = crate::primitives::layout_metrics::LayoutCtxMeasure {
            ctx,
            role: crate::FontRole::Chrome,
        };
        // A `Wrap` bar measures each line at one text row; the other two
        // keep the whole rect as the single line's height — the same
        // split every pixel wrapper's own `*_layout_with_overflow` makes.
        let line_height = if options.overflow == super::ToolbarOverflow::Wrap {
            ctx.line_height
        } else {
            rect.height
        };
        let mut layout = self.layout_with_overflow(
            0.0,
            0.0,
            rect.width,
            line_height,
            options,
            &self.overflow_priorities,
            |btn| ToolbarItemMeasure::new(super::measure_button(&measure, btn)),
        );
        // Rows a `Wrap` layout placed past the requested height are kept
        // in `visible_items` (so a host can read `cross_extent` and grow
        // its rect) but marked un-clickable, matching what `paint`'s own
        // clip actually puts on screen.
        layout.demote_lines_past(options, rect.height);
        ToolbarPrimitiveLayout {
            area: rect,
            layout,
            valign: self.valign,
            caret_extent: MENU_BUTTON_CARET_PX,
        }
    }

    /// Runs the shared pixel painter every GUI wrapper already paints
    /// through, at the already-resolved layout translated into surface
    /// coordinates.
    fn paint(
        &self,
        layout: &Self::Layout,
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
            // `Primitive::paint`'s fixed signature carries no per-call
            // interaction-state channel (see that trait's own doc), so
            // the hover/press pair comes from the "no such state"
            // defaults `PaintState` already falls back to — named
            // through `ToolbarState` rather than a bare `None`/`None`
            // literal pair. The descriptor's own `focused_index` is the
            // one piece of paint state that is not pointer-derived, and
            // the painter reads it off `self`.
            let state = ToolbarState::default();
            let translated =
                translate_toolbar_layout(layout.layout.clone(), layout.area.x, layout.area.y);
            super::native_surface_paint::paint(
                self,
                &translated,
                surface,
                theme,
                state.hovered_id(self),
                state.pressed_id(self),
                super::ToolbarPaintOptions {
                    valign: layout.valign,
                },
            );
            let _ = style;
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

    /// Delegates to the same `super::cell_paint` entry point the legacy
    /// `crate::tui::draw_toolbar*` wrappers and
    /// `TuiBackend::draw_toolbar_with_options` themselves call, picked by
    /// what the descriptor asks for — so painting a `Toolbar` through
    /// this contract is byte-for-byte identical to painting it through
    /// the legacy `Backend` pair by construction, not by separately
    /// maintained parity.
    ///
    /// A plain `Clip`/`Horizontal` bar goes through
    /// [`super::cell_paint::draw_toolbar_with_options`], the only cell
    /// painter that honours [`Toolbar::valign`]; a wrapping, menu-overflow
    /// or vertical bar goes through
    /// [`super::cell_paint::draw_toolbar_with_overflow`], where every
    /// line is one row tall and `valign` has nothing to resolve.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &Self::Layout,
        buf: &mut ratatui::buffer::Buffer,
        theme: &crate::theme::Theme,
    ) {
        let area = crate::tui::backend::q_rect_to_ratatui(layout.area);
        let state = ToolbarState::default();
        let options = self.layout_options();
        let hovered = state.hovered_id(self);
        let pressed = state.pressed_id(self);
        if options.overflow == super::ToolbarOverflow::Clip
            && options.orientation == super::ToolbarOrientation::Horizontal
        {
            let _ = super::cell_paint::draw_toolbar_with_options(
                buf,
                area,
                self,
                theme,
                hovered,
                pressed,
                super::ToolbarPaintOptions {
                    valign: layout.valign,
                },
            );
        } else {
            let _ = super::cell_paint::draw_toolbar_with_overflow(
                buf,
                area,
                self,
                theme,
                hovered,
                pressed,
                options,
                &self.overflow_priorities,
            );
        }
    }
}

// ── Sub-parts ───────────────────────────────────────────────────────

/// One addressable part of a painted [`Toolbar`] — what
/// [`Interactive::sub_regions`] names, so a driver can target "the
/// dropdown arrow of item 3" and a binding layer can turn a hit back
/// into a part it can name.
///
/// Every index is an index into [`Toolbar::buttons`] (a `Group` child
/// additionally carries its own index within that group's `items`),
/// never into the resolved layout's `visible_items` — a clipped or
/// dropped item shifts the latter but not the descriptor.
///
/// Round-trips through [`Self::sub_target`]/[`Self::from_sub_target`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolbarSubPart {
    /// A clickable item's slot — an [`ToolbarButton::Action`], a
    /// [`ToolbarButton::Field`], or a [`ToolbarButton::MenuButton`]'s
    /// body.
    Button(usize),
    /// One [`ToolbarButton::Group`] child's own slot inside the group's
    /// shared bracket.
    GroupChild {
        /// The owning `Group`'s index in [`Toolbar::buttons`].
        item: usize,
        /// The child's index in that `Group`'s own `items`.
        child: usize,
    },
    /// A [`ToolbarButton::MenuButton`]'s trailing `▾` affordance.
    DropdownArrow(usize),
    /// The trailing `»` affordance a
    /// [`super::ToolbarOverflow::Menu`] layout paints when it dropped
    /// items.
    OverflowChevron,
}

impl ToolbarSubPart {
    /// This part's [`SubTarget`] tag: a kind in the high bits, the item
    /// index in the middle, a `Group` child index in the low bits — the
    /// same pack-into-one-`u64` convention
    /// [`crate::interactive::FocusStop`]'s own cell-packing example
    /// uses.
    pub fn sub_target(self) -> SubTarget {
        const MASK: u64 = 0xff_ffff;
        let (kind, item, child) = match self {
            Self::Button(i) => (1u64, i as u64, 0),
            Self::GroupChild { item, child } => (2, item as u64, child as u64),
            Self::DropdownArrow(i) => (3, i as u64, 0),
            Self::OverflowChevron => (4, 0, 0),
        };
        SubTarget((kind << 48) | ((item & MASK) << 24) | (child & MASK))
    }

    /// Decode a [`Self::sub_target`] tag back into a part, or `None` for
    /// a tag this widget never produced (another widget's sub-target).
    pub fn from_sub_target(sub: SubTarget) -> Option<Self> {
        const MASK: u64 = 0xff_ffff;
        let item = ((sub.0 >> 24) & MASK) as usize;
        let child = (sub.0 & MASK) as usize;
        match sub.0 >> 48 {
            1 => Some(Self::Button(item)),
            2 => Some(Self::GroupChild { item, child }),
            3 => Some(Self::DropdownArrow(item)),
            4 => Some(Self::OverflowChevron),
            _ => None,
        }
    }
}

/// The absolute centre of one toolbar sub-part in the bar `bar` — the
/// point a click must land on to hit it, resolved against the frame's
/// painted-layout record ([`crate::primitive::PaintedWidgets`]), which
/// every bar painted through
/// [`crate::backend::DrawsPrimitives::draw_dyn`] registers itself in.
///
/// `None` when `bar` didn't paint this frame, or when the named part
/// isn't on screen (an item clipped against the bar's trailing edge, a
/// dropdown arrow on a bar with no menu button, an absent `»`).
pub fn toolbar_part_center(bar: &WidgetId, part: ToolbarSubPart) -> Option<(f32, f32)> {
    crate::primitive::painted_sub_center(bar, part.sub_target())
}

/// Every addressable sub-part of a resolved toolbar layout, paired with
/// the [`SubTarget`] that names it, in the layout's own bar-relative
/// space.
///
/// Most-specific first: a menu button's dropdown arrow before the button
/// body that contains it, and a `Group`'s children before nothing at all
/// (the group slot itself is never clickable — see
/// [`ToolbarLayout::hit_test`]), so the first rect containing a point is
/// the right answer.
fn toolbar_sub_regions(layout: &ToolbarPrimitiveLayout) -> Vec<(Rect, SubTarget)> {
    let mut out = Vec::new();
    for vis in &layout.layout.visible_items {
        if vis.bounds.width <= 0.0 || vis.bounds.height <= 0.0 {
            continue;
        }
        match vis.kind {
            ToolbarItemKind::Group => {
                for child in &vis.group_children {
                    if child.bounds.width <= 0.0 || !child.clickable {
                        continue;
                    }
                    out.push((
                        child.bounds,
                        ToolbarSubPart::GroupChild {
                            item: vis.item_idx,
                            child: child.item_idx,
                        }
                        .sub_target(),
                    ));
                }
            }
            ToolbarItemKind::MenuButton => {
                let caret = layout.caret_extent.min(vis.bounds.width);
                if caret > 0.0 {
                    out.push((
                        Rect::new(
                            vis.bounds.x + vis.bounds.width - caret,
                            vis.bounds.y,
                            caret,
                            vis.bounds.height,
                        ),
                        ToolbarSubPart::DropdownArrow(vis.item_idx).sub_target(),
                    ));
                }
                out.push((
                    vis.bounds,
                    ToolbarSubPart::Button(vis.item_idx).sub_target(),
                ));
            }
            ToolbarItemKind::Overflow => {
                out.push((vis.bounds, ToolbarSubPart::OverflowChevron.sub_target()));
            }
            ToolbarItemKind::Action | ToolbarItemKind::Field if vis.clickable => {
                out.push((
                    vis.bounds,
                    ToolbarSubPart::Button(vis.item_idx).sub_target(),
                ));
            }
            // A separator, an inline label and a spacer address nothing:
            // none of them is clickable or focusable.
            _ => {}
        }
    }
    out
}

// ── Interactive ─────────────────────────────────────────────────────

impl WidgetAction for ToolbarEvent {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        crate::UiEvent::Toolbar(id, self)
    }
}

/// [`Interactive::State`] for [`Toolbar`] — the live counterpart of the
/// `hovered_id` / `pressed_id` pair every `draw_toolbar*` entry point
/// takes from its host (what `compose::ToolbarHoverTracker` tracks for a
/// host that drives the legacy `Backend` path), plus the keyboard cursor
/// [`Toolbar::focused_index`] paints.
///
/// Both pointer fields are stored twice over, as an index *and* as an
/// id, because the two consumers want different keys: the painters
/// highlight by [`WidgetId`] (a `Group` child has its own id and no
/// top-level index of its own), while
/// [`crate::primitive::PaintState`]'s shared hover/press channel is
/// index-addressed.
///
/// `#[non_exhaustive]`: a future field add here is additive.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolbarState {
    /// [`Toolbar::buttons`] index of the slot under the pointer, or
    /// `None` when the pointer is off every item.
    pub hovered_item: Option<usize>,
    /// Id of the button under the pointer — a `Group` child's own id
    /// when the hovered slot is a group, `None` over a separator, label,
    /// spacer or the `»` affordance (which has no id a host could hand
    /// back).
    pub hovered_id: Option<WidgetId>,
    /// [`Toolbar::buttons`] index of the slot the pointer is holding
    /// down.
    pub pressed_item: Option<usize>,
    /// Id of the button the pointer is holding down.
    pub pressed_id: Option<WidgetId>,
    /// The item Left/Right (or Up/Down) has moved the keyboard cursor
    /// to. `None` until the first arrow press. A host threads this back
    /// onto the next frame's [`Toolbar::focused_index`] to paint the
    /// focus highlight.
    pub cursor: Option<usize>,
}

/// Index-addressed hover/press only: the `*_id` halves have no
/// counterpart on this trait, so a painter that highlights by
/// [`WidgetId`] reads [`ToolbarState::hovered_id`] /
/// [`ToolbarState::pressed_id`] directly.
impl PaintState for ToolbarState {
    fn hovered_idx(&self) -> Option<usize> {
        self.hovered_item
    }

    fn pressed_idx(&self) -> Option<usize> {
        self.pressed_item
    }
}

impl ToolbarState {
    /// The hovered button's id, as the painters want it — `None` unless
    /// it still names a button in `bar` (a stale id from a previous
    /// frame's longer button list highlights nothing).
    pub fn hovered_id<'a>(&'a self, bar: &Toolbar) -> Option<&'a WidgetId> {
        self.hovered_id.as_ref().filter(|id| bar.contains_id(id))
    }

    /// The pressed button's id, as the painters want it — same
    /// staleness filter as [`Self::hovered_id`].
    pub fn pressed_id<'a>(&'a self, bar: &Toolbar) -> Option<&'a WidgetId> {
        self.pressed_id.as_ref().filter(|id| bar.contains_id(id))
    }
}

impl Interactive for Toolbar {
    type State = ToolbarState;
    type Action = ToolbarEvent;

    /// One focus stop as a whole: the bar's own arrow keys move its
    /// internal cursor, so tab order sees one stop, not one per button.
    /// [`FocusSpec::None`] for a bar with nothing activatable on it.
    fn focus(&self) -> FocusSpec {
        if self.enabled_item_indices().is_empty() {
            FocusSpec::None
        } else {
            FocusSpec::Stop
        }
    }

    fn sub_regions(&self, layout: &Self::Layout) -> Vec<(Rect, SubTarget)> {
        toolbar_sub_regions(layout)
    }

    fn handle(
        &self,
        state: &mut ToolbarState,
        event: &RoutedEvent,
        layout: &Self::Layout,
        cx: &mut EventCx,
    ) -> Handled<ToolbarEvent> {
        match event {
            // Both kinds carry a live `local` position, and both mean
            // "the button under the cursor may have changed": a
            // `Move` while this bar holds the pointer capture, an
            // `Enter` when the cursor crosses into the strip (the only
            // within-bar position the routing engine delivers to an
            // uncaptured widget — see `runtime::route_widget_hover`).
            RoutedEvent::Pointer {
                kind: PointerKind::Move | PointerKind::Enter,
                local,
                ..
            } => {
                let hit = layout.hit_test(local.x, local.y);
                let item = self.item_index_for_hit(&hit, &layout.layout);
                let id = super::hover_target(&hit);
                if state.hovered_item == item && state.hovered_id == id {
                    return Handled::Ignored;
                }
                state.hovered_item = item;
                state.hovered_id = id;
                Handled::Consumed
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Leave,
                ..
            } => {
                if state.hovered_item.is_none()
                    && state.hovered_id.is_none()
                    && state.pressed_id.is_none()
                {
                    return Handled::Ignored;
                }
                state.hovered_item = None;
                state.hovered_id = None;
                state.pressed_item = None;
                state.pressed_id = None;
                Handled::Consumed
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                local,
                ..
            } => {
                let hit = layout.hit_test(local.x, local.y);
                let item = self.item_index_for_hit(&hit, &layout.layout);
                state.pressed_item = item;
                state.pressed_id = super::hover_target(&hit);
                match hit {
                    ToolbarHit::Button(id) => {
                        // A click parks the keyboard cursor on what it
                        // activated, so a following Enter repeats it —
                        // the same click-then-keyboard continuity
                        // `TabBar`'s own contract has.
                        cx.request_focus(None);
                        if let Some(idx) = item {
                            state.cursor = Some(idx);
                        }
                        match self.click(id) {
                            Some(event) => Handled::Event(event),
                            None => Handled::Consumed,
                        }
                    }
                    // A field click is a focus request: the host owns
                    // the embedded `TextField`'s own `has_focus`, and
                    // the field's keystrokes are routed to it rather
                    // than through this bar — see
                    // `ToolbarButton::Field`'s own doc.
                    ToolbarHit::Field(id) => {
                        let sub = item
                            .map(|idx| ToolbarSubPart::Button(idx).sub_target())
                            .or_else(|| {
                                self.index_of(&id)
                                    .map(|idx| ToolbarSubPart::Button(idx).sub_target())
                            });
                        cx.request_focus(sub);
                        Handled::Consumed
                    }
                    ToolbarHit::MenuButton(id) => {
                        cx.request_focus(None);
                        if let Some(idx) = item {
                            state.cursor = Some(idx);
                        }
                        Handled::Event(ToolbarEvent::MenuRequested { id })
                    }
                    ToolbarHit::Overflow => {
                        cx.request_focus(None);
                        Handled::Event(ToolbarEvent::OverflowRequested {
                            id: super::toolbar_overflow_widget_id(),
                        })
                    }
                    // A separator, an inline label, a spacer, a disabled
                    // action: inside the bar but not actionable, so the
                    // click still bubbles to whatever is behind it.
                    ToolbarHit::Empty => Handled::Ignored,
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Up(MouseButton::Left),
                ..
            } => {
                if state.pressed_item.is_none() && state.pressed_id.is_none() {
                    return Handled::Ignored;
                }
                state.pressed_item = None;
                state.pressed_id = None;
                Handled::Consumed
            }
            RoutedEvent::Key(KeyEvent { key, .. }) => self.handle_key_routed(state, key),
            // The assistive-technology default action activates whatever
            // the keyboard cursor already names, falling back to the
            // first enabled item — the same "describe what a real
            // keyboard gesture would do" contract
            // `a11y::action::route_action` states.
            RoutedEvent::Action(A11yAction::Click) => match self.activatable_cursor(state.cursor) {
                Some(idx) => self.activate_index(idx),
                None => Handled::Ignored,
            },
            _ => Handled::Ignored,
        }
    }

    /// `Click` whenever anything on the bar can be activated — the
    /// gesture [`Self::handle`]'s own [`A11yAction::Click`] arm serves.
    fn a11y_actions(&self, state: &ToolbarState) -> Vec<A11yAction> {
        match self.activatable_cursor(state.cursor) {
            Some(_) => vec![A11yAction::Click],
            None => Vec::new(),
        }
    }
}

impl Toolbar {
    /// Indices into [`Self::buttons`] the keyboard cursor may land on:
    /// every enabled [`ToolbarButton::Action`], [`ToolbarButton::Field`]
    /// and [`ToolbarButton::MenuButton`], skipping separators, inline
    /// labels, spacers and disabled actions.
    ///
    /// A [`ToolbarButton::Group`] is not a stop of its own (it is never
    /// directly clickable — see [`ToolbarLayout::hit_test`]); its
    /// enabled children are not stops either, since
    /// [`Self::focused_index`] addresses top-level slots only.
    fn enabled_item_indices(&self) -> Vec<usize> {
        self.buttons
            .iter()
            .enumerate()
            .filter(|(_, btn)| {
                matches!(
                    btn,
                    ToolbarButton::Action { enabled: true, .. }
                        | ToolbarButton::Field { .. }
                        | ToolbarButton::MenuButton { .. }
                )
            })
            .map(|(idx, _)| idx)
            .collect()
    }

    /// `true` when `id` names an [`ToolbarButton::Action`],
    /// [`ToolbarButton::Field`] or [`ToolbarButton::MenuButton`] in this
    /// bar — including a [`ToolbarButton::Group`] child.
    fn contains_id(&self, id: &WidgetId) -> bool {
        self.buttons.iter().any(|btn| {
            match btn {
            ToolbarButton::Action { id: btn_id, .. }
            | ToolbarButton::Field { id: btn_id, .. }
            | ToolbarButton::MenuButton { id: btn_id, .. } => btn_id == id,
            ToolbarButton::Group { items, .. } => items.iter().any(|child| {
                matches!(child, ToolbarButton::Action { id: child_id, .. } if child_id == id)
            }),
            _ => false,
        }
        })
    }

    /// The top-level [`Self::buttons`] index `id` sits at, or the index
    /// of the [`ToolbarButton::Group`] that holds it. `None` for an id
    /// this bar doesn't carry.
    fn index_of(&self, id: &WidgetId) -> Option<usize> {
        self.buttons.iter().position(|btn| {
            match btn {
            ToolbarButton::Action { id: btn_id, .. }
            | ToolbarButton::Field { id: btn_id, .. }
            | ToolbarButton::MenuButton { id: btn_id, .. } => btn_id == id,
            ToolbarButton::Group { items, .. } => items.iter().any(|child| {
                matches!(child, ToolbarButton::Action { id: child_id, .. } if child_id == id)
            }),
            _ => false,
        }
        })
    }

    /// Which [`Self::buttons`] slot `hit` landed in: the `»`
    /// affordance's own layout entry carries
    /// [`super::TOOLBAR_OVERFLOW_ITEM_IDX`] rather than a real index, so
    /// that one resolves to `None`.
    fn item_index_for_hit(&self, hit: &ToolbarHit, layout: &ToolbarLayout) -> Option<usize> {
        match hit {
            ToolbarHit::Button(id) | ToolbarHit::Field(id) | ToolbarHit::MenuButton(id) => {
                self.index_of(id)
            }
            ToolbarHit::Overflow => {
                let _ = layout;
                None
            }
            ToolbarHit::Empty => None,
        }
    }

    /// The cursor index to activate on Enter/Space or an a11y `Click`:
    /// `cursor` when it still names an activatable slot, otherwise the
    /// first one on the bar.
    fn activatable_cursor(&self, cursor: Option<usize>) -> Option<usize> {
        let stops = self.enabled_item_indices();
        match cursor {
            Some(idx) if stops.contains(&idx) => Some(idx),
            _ => stops.first().copied(),
        }
    }

    /// The event activating the slot at `idx` stands for — an `Action`'s
    /// own [`Self::click`] event, a `MenuButton`'s
    /// [`ToolbarEvent::MenuRequested`], a `Field`'s
    /// [`ToolbarEvent::FieldCommitted`].
    fn activate_index(&self, idx: usize) -> Handled<ToolbarEvent> {
        match self.buttons.get(idx) {
            Some(ToolbarButton::Action { id, .. }) => match self.click(*id) {
                Some(event) => Handled::Event(event),
                None => Handled::Ignored,
            },
            Some(ToolbarButton::MenuButton { id, .. }) => {
                Handled::Event(ToolbarEvent::MenuRequested { id: *id })
            }
            Some(ToolbarButton::Field { id, field, .. }) => {
                Handled::Event(ToolbarEvent::FieldCommitted {
                    id: *id,
                    value: field.value.clone(),
                })
            }
            _ => Handled::Ignored,
        }
    }

    /// Dispatch one keystroke against live [`ToolbarState`].
    ///
    /// Left/Up move the cursor to the previous enabled item and
    /// Right/Down to the next (both axes, since the same descriptor
    /// paints a horizontal strip and a vertical rail), Home/End jump to
    /// the first/last, and Enter/Space activate the cursor's item. Every
    /// move runs through [`FlatNav`] over
    /// [`Self::enabled_item_indices`], so separators, inline labels,
    /// spacers and disabled actions are skipped rather than landed on.
    /// Any other key stays [`Handled::Ignored`] so an app's own hotkeys
    /// still see it.
    fn handle_key_routed(&self, state: &mut ToolbarState, key: &Key) -> Handled<ToolbarEvent> {
        let stops = self.enabled_item_indices();
        if stops.is_empty() {
            return Handled::Ignored;
        }
        let nav = match key {
            Key::Named(NamedKey::Left) | Key::Named(NamedKey::Up) => NavKey::Up,
            Key::Named(NamedKey::Right) | Key::Named(NamedKey::Down) => NavKey::Down,
            Key::Named(NamedKey::Home) => NavKey::Home,
            Key::Named(NamedKey::End) => NavKey::End,
            Key::Named(NamedKey::Enter) | Key::Char(' ') => {
                return match self.activatable_cursor(state.cursor) {
                    Some(idx) => {
                        state.cursor = Some(idx);
                        self.activate_index(idx)
                    }
                    None => Handled::Ignored,
                };
            }
            _ => return Handled::Ignored,
        };
        let moved = FlatNav::mv(self.buttons.len(), state.cursor, nav, false, |idx| {
            stops.contains(&idx)
        });
        match moved {
            Some(idx) if Some(idx) != state.cursor => {
                state.cursor = Some(idx);
                Handled::Consumed
            }
            // Already parked on the end of the run the key moves toward:
            // the bar still owns the key (it is navigating itself), it
            // just has nowhere further to go.
            Some(_) => Handled::Consumed,
            None => Handled::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Point;
    use crate::interactive::EventHost;
    use crate::types::Modifiers;
    use crate::PointerShape;

    /// Minimal [`EventHost`] mock — the same shape
    /// `crate::primitives::tab_bar`'s own module tests use.
    #[derive(Debug)]
    struct MockHost {
        focus_requests: Vec<Option<SubTarget>>,
        emitted: Vec<crate::UiEvent>,
        units: Units,
    }

    impl MockHost {
        fn new(units: Units) -> Self {
            Self {
                focus_requests: Vec::new(),
                emitted: Vec::new(),
                units,
            }
        }
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
            self.units
        }
    }

    fn action(id: &str, label: &str, enabled: bool) -> ToolbarButton {
        ToolbarButton::Action {
            id: WidgetId::new(id),
            label: label.to_string(),
            icon: None,
            key_hint: None,
            enabled,
            is_active: false,
            tooltip: String::new(),
            spec: None,
        }
    }

    fn menu_button(id: &str, label: &str) -> ToolbarButton {
        ToolbarButton::MenuButton {
            id: WidgetId::new(id),
            label: label.to_string(),
            icon: None,
            menu: vec![],
        }
    }

    /// `[ A ]` `│` `[ B ]` (B disabled) `[ More ▾ ]` — one of every
    /// keyboard-relevant shape, in bar order.
    fn bar() -> Toolbar {
        Toolbar::new(WidgetId::new("tb")).with_buttons(vec![
            action("tb:a", "A", true),
            ToolbarButton::Separator,
            action("tb:b", "B", false),
            menu_button("tb:more", "More"),
        ])
    }

    /// Whole-cell metrics plus a 40x1 strip: [`LayoutCtx::units`] reports
    /// [`Units::Cells`], so [`Primitive::layout`] takes its TUI branch.
    #[cfg(feature = "tui")]
    fn cell_layout_of(bar: &Toolbar) -> ToolbarPrimitiveLayout {
        Primitive::layout(
            bar,
            &LayoutCtx::new(1.0, 1.0, false),
            Rect::new(3.0, 2.0, 40.0, 1.0),
        )
    }

    /// Pixel metrics plus a 400x24 strip — the regime every pixel
    /// backend lays out in.
    fn pixel_layout_of(bar: &Toolbar) -> ToolbarPrimitiveLayout {
        Primitive::layout(
            bar,
            &LayoutCtx::new(24.0, 8.0, false),
            Rect::new(10.0, 5.0, 400.0, 24.0),
        )
    }

    /// Both unit regimes this build can resolve — the pixel one always,
    /// the cell one when `tui` is compiled in.
    fn layouts(bar: &Toolbar) -> Vec<ToolbarPrimitiveLayout> {
        #[allow(unused_mut)] // `mut` is only needed with `tui` compiled in
        let mut out = vec![pixel_layout_of(bar)];
        #[cfg(feature = "tui")]
        out.push(cell_layout_of(bar));
        out
    }

    fn pointer(kind: PointerKind, x: f32, y: f32) -> RoutedEvent {
        RoutedEvent::Pointer {
            kind,
            sub: None,
            local: Point { x, y },
            mods: Modifiers::default(),
        }
    }

    fn key(named: NamedKey) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    #[test]
    fn layout_is_bar_relative_in_every_unit_regime() {
        let bar = bar();
        for layout in layouts(&bar) {
            let first = layout
                .layout
                .visible_items
                .first()
                .expect("the first action lays out");
            assert_eq!(
                (first.bounds.x, first.bounds.y),
                (0.0, 0.0),
                "bounds start at the widget's own origin, not {:?}",
                layout.area,
            );
            // The absolute area is still recorded, for `paint` to
            // translate by.
            assert!(layout.area.x > 0.0 && layout.area.y > 0.0);
        }
    }

    #[test]
    fn hit_test_resolves_the_first_action_in_every_unit_regime() {
        let bar = bar();
        for layout in layouts(&bar) {
            assert_eq!(
                layout.hit_test(1.0, 0.0),
                ToolbarHit::Button(WidgetId::new("tb:a")),
            );
        }
    }

    #[test]
    fn sub_regions_put_the_dropdown_arrow_before_the_button_body() {
        let bar = bar();
        for layout in layouts(&bar) {
            let regions = Interactive::sub_regions(&bar, &layout);
            let parts: Vec<ToolbarSubPart> = regions
                .iter()
                .filter_map(|(_, sub)| ToolbarSubPart::from_sub_target(*sub))
                .collect();
            let arrow = parts
                .iter()
                .position(|p| matches!(p, ToolbarSubPart::DropdownArrow(3)))
                .expect("the menu button's arrow is addressable");
            let body = parts
                .iter()
                .position(|p| matches!(p, ToolbarSubPart::Button(3)))
                .expect("the menu button's body is addressable");
            assert!(arrow < body, "most-specific first: {parts:?}");
            // A separator and a disabled action address nothing.
            assert!(!parts.contains(&ToolbarSubPart::Button(1)));
            assert!(!parts.contains(&ToolbarSubPart::Button(2)));
        }
    }

    #[test]
    fn sub_parts_round_trip_through_sub_target() {
        for part in [
            ToolbarSubPart::Button(7),
            ToolbarSubPart::GroupChild { item: 2, child: 5 },
            ToolbarSubPart::DropdownArrow(3),
            ToolbarSubPart::OverflowChevron,
        ] {
            assert_eq!(
                ToolbarSubPart::from_sub_target(part.sub_target()),
                Some(part),
            );
        }
        // Another widget's tag decodes to nothing.
        assert_eq!(ToolbarSubPart::from_sub_target(SubTarget(0)), None);
    }

    /// Both position-carrying pointer kinds track hover: a `Move` while
    /// the bar holds the capture, an `Enter` when the cursor crosses
    /// into the strip.
    #[test]
    fn a_positioned_pointer_tracks_the_hovered_button_and_leave_clears_it() {
        for kind in [PointerKind::Move, PointerKind::Enter] {
            pointer_hover_round_trip(kind);
        }
    }

    fn pointer_hover_round_trip(kind: PointerKind) {
        let bar = bar();
        let layout = pixel_layout_of(&bar);
        let mut host = MockHost::new(Units::Pixels);
        let mut state = ToolbarState::default();

        let mut cx = EventCx::new(bar.id, &mut host);
        let out = Interactive::handle(&bar, &mut state, &pointer(kind, 2.0, 2.0), &layout, &mut cx);
        assert!(out.is_consumed(), "{kind:?} tracks hover");
        assert_eq!(state.hovered_item, Some(0));
        assert_eq!(state.hovered_id, Some(WidgetId::new("tb:a")));
        assert_eq!(state.hovered_idx(), Some(0), "PaintState reads the index");
        assert_eq!(state.hovered_id(&bar), Some(&WidgetId::new("tb:a")));

        // A second position inside the same button changes nothing.
        let mut cx = EventCx::new(bar.id, &mut host);
        assert!(
            Interactive::handle(&bar, &mut state, &pointer(kind, 3.0, 2.0), &layout, &mut cx)
                .is_ignored()
        );

        let mut cx = EventCx::new(bar.id, &mut host);
        assert!(Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Leave, 0.0, 0.0),
            &layout,
            &mut cx
        )
        .is_consumed());
        assert_eq!(state.hovered_id, None);
        assert_eq!(state.hovered_idx(), None);
    }

    #[test]
    fn a_click_activates_the_button_parks_the_cursor_and_presses_it() {
        let bar = bar();
        let layout = pixel_layout_of(&bar);
        let mut host = MockHost::new(Units::Pixels);
        let mut state = ToolbarState::default();

        let mut cx = EventCx::new(bar.id, &mut host);
        let out = Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Down(MouseButton::Left), 2.0, 2.0),
            &layout,
            &mut cx,
        );
        assert_eq!(
            out,
            Handled::Event(ToolbarEvent::ButtonClicked {
                id: WidgetId::new("tb:a"),
                is_active: true,
            }),
        );
        assert_eq!(state.cursor, Some(0), "a click parks the keyboard cursor");
        assert_eq!(state.pressed_idx(), Some(0));
        assert_eq!(host.focus_requests, vec![None]);

        let mut cx = EventCx::new(bar.id, &mut host);
        assert!(Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Up(MouseButton::Left), 2.0, 2.0),
            &layout,
            &mut cx,
        )
        .is_consumed());
        assert_eq!(state.pressed_idx(), None);
    }

    #[test]
    fn a_click_on_a_menu_button_requests_its_dropdown() {
        let bar = bar();
        let layout = pixel_layout_of(&bar);
        let more = layout
            .layout
            .visible_items
            .iter()
            .find(|v| v.kind == ToolbarItemKind::MenuButton)
            .expect("the menu button lays out");
        let mut host = MockHost::new(Units::Pixels);
        let mut state = ToolbarState::default();
        let mut cx = EventCx::new(bar.id, &mut host);
        let out = Interactive::handle(
            &bar,
            &mut state,
            &pointer(
                PointerKind::Down(MouseButton::Left),
                more.bounds.x + 1.0,
                more.bounds.y + 1.0,
            ),
            &layout,
            &mut cx,
        );
        assert_eq!(
            out,
            Handled::Event(ToolbarEvent::MenuRequested {
                id: WidgetId::new("tb:more"),
            }),
        );
    }

    #[test]
    fn a_click_on_dead_space_still_bubbles() {
        let bar = bar();
        let layout = pixel_layout_of(&bar);
        let mut host = MockHost::new(Units::Pixels);
        let mut state = ToolbarState::default();
        let mut cx = EventCx::new(bar.id, &mut host);
        assert!(Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Down(MouseButton::Left), 399.0, 2.0),
            &layout,
            &mut cx,
        )
        .is_ignored());
    }

    #[test]
    fn arrow_keys_walk_enabled_items_and_skip_separators_and_disabled_actions() {
        let bar = bar();
        let layout = pixel_layout_of(&bar);
        let mut host = MockHost::new(Units::Pixels);
        let mut state = ToolbarState::default();

        // Right from nothing seeds the first enabled item.
        let mut cx = EventCx::new(bar.id, &mut host);
        assert!(
            Interactive::handle(&bar, &mut state, &key(NamedKey::Right), &layout, &mut cx)
                .is_consumed()
        );
        assert_eq!(state.cursor, Some(0));

        // Every navigation step claims the key, and the separator (1)
        // and the disabled action (2) are skipped rather than landed on.
        for (named, expected) in [
            (NamedKey::Right, 3),
            // Clamped at the end, not wrapped — the bar still owns the
            // key, it just has nowhere further to go.
            (NamedKey::Right, 3),
            (NamedKey::Left, 0),
            (NamedKey::End, 3),
            (NamedKey::Home, 0),
        ] {
            let mut cx = EventCx::new(bar.id, &mut host);
            assert!(
                Interactive::handle(&bar, &mut state, &key(named), &layout, &mut cx).is_consumed(),
                "{named:?} is the bar's own navigation key",
            );
            assert_eq!(state.cursor, Some(expected), "after {named:?}");
        }
    }

    #[test]
    fn enter_activates_the_cursor_and_an_unbound_key_bubbles() {
        let bar = bar();
        let layout = pixel_layout_of(&bar);
        let mut host = MockHost::new(Units::Pixels);
        let mut state = ToolbarState {
            cursor: Some(3),
            ..Default::default()
        };

        let mut cx = EventCx::new(bar.id, &mut host);
        assert_eq!(
            Interactive::handle(&bar, &mut state, &key(NamedKey::Enter), &layout, &mut cx),
            Handled::Event(ToolbarEvent::MenuRequested {
                id: WidgetId::new("tb:more"),
            }),
        );

        let mut cx = EventCx::new(bar.id, &mut host);
        assert!(Interactive::handle(
            &bar,
            &mut state,
            &RoutedEvent::Key(KeyEvent {
                key: Key::Char('x'),
                modifiers: Modifiers::default(),
                repeat: false,
            }),
            &layout,
            &mut cx,
        )
        .is_ignored());
    }

    #[test]
    fn the_a11y_click_action_activates_what_the_cursor_names() {
        let bar = bar();
        let layout = pixel_layout_of(&bar);
        let mut host = MockHost::new(Units::Pixels);
        let mut state = ToolbarState::default();
        assert_eq!(
            Interactive::a11y_actions(&bar, &state),
            vec![A11yAction::Click]
        );

        let mut cx = EventCx::new(bar.id, &mut host);
        assert_eq!(
            Interactive::handle(
                &bar,
                &mut state,
                &RoutedEvent::Action(A11yAction::Click),
                &layout,
                &mut cx,
            ),
            Handled::Event(ToolbarEvent::ButtonClicked {
                id: WidgetId::new("tb:a"),
                is_active: true,
            }),
        );
    }

    #[test]
    fn a_bar_with_nothing_activatable_is_no_focus_stop() {
        let inert = Toolbar::new(WidgetId::new("tb")).with_buttons(vec![
            ToolbarButton::Separator,
            action("tb:off", "Off", false),
            ToolbarButton::Label {
                text: "2 of 5".into(),
                fg: None,
            },
        ]);
        assert_eq!(Interactive::focus(&inert), FocusSpec::None);
        assert!(Interactive::a11y_actions(&inert, &ToolbarState::default()).is_empty());
        assert_eq!(Interactive::focus(&bar()), FocusSpec::Stop);
    }

    #[test]
    fn a_stale_hover_id_highlights_nothing() {
        let bar = bar();
        let state = ToolbarState {
            hovered_id: Some(WidgetId::new("tb:gone")),
            ..Default::default()
        };
        assert_eq!(state.hovered_id(&bar), None);
    }

    /// The overflow chevron is the one layout entry with no
    /// `Toolbar::buttons` index of its own, and it answers
    /// `OverflowRequested` rather than a per-button event. The layout
    /// comes from `Primitive::layout` reading
    /// `Toolbar::overflow`/`Toolbar::overflow_priorities` off the
    /// descriptor, which is the only input a host gives it.
    #[test]
    fn the_overflow_chevron_requests_the_overflow_menu() {
        let crowded = Toolbar::new(WidgetId::new("tb"))
            .with_buttons(vec![
                action("tb:a", "Alpha", true),
                action("tb:b", "Bravo", true),
                action("tb:c", "Charlie", true),
            ])
            .with_overflow(super::super::ToolbarOverflow::Menu);
        // Whole-cell metrics: 24 cells is too narrow for the three
        // bracketed labels in either unit regime, so the `Menu` ladder
        // drops one and grows the affordance.
        let primitive_layout = Primitive::layout(
            &crowded,
            &LayoutCtx::new(1.0, 1.0, false),
            Rect::new(0.0, 0.0, 24.0, 1.0),
        );
        let chevron = primitive_layout
            .layout
            .visible_items
            .iter()
            .find(|v| v.kind == ToolbarItemKind::Overflow)
            .expect("a `Menu` overflow layout that dropped items paints the chevron")
            .clone();
        let regions = Interactive::sub_regions(&crowded, &primitive_layout);
        assert!(regions
            .iter()
            .any(|(_, sub)| *sub == ToolbarSubPart::OverflowChevron.sub_target()));

        let mut host = MockHost::new(Units::Cells);
        let mut state = ToolbarState::default();
        let mut cx = EventCx::new(crowded.id, &mut host);
        assert_eq!(
            Interactive::handle(
                &crowded,
                &mut state,
                &pointer(
                    PointerKind::Down(MouseButton::Left),
                    chevron.bounds.x + 1.0,
                    0.0,
                ),
                &primitive_layout,
                &mut cx,
            ),
            Handled::Event(ToolbarEvent::OverflowRequested {
                id: super::super::toolbar_overflow_widget_id(),
            }),
        );
    }

    /// Three bracketed labels too wide for the strip, so an overflow
    /// knob on the descriptor has something to do.
    fn crowded_bar() -> Toolbar {
        Toolbar::new(WidgetId::new("tb")).with_buttons(vec![
            action("tb:a", "Alpha", true),
            action("tb:b", "Bravo", true),
            action("tb:c", "Charlie", true),
        ])
    }

    /// `Toolbar::overflow` is honoured by `Primitive::layout`: a `Wrap`
    /// descriptor reports more cross-axis room than the default `Clip`
    /// one, because the items that don't fit went onto a second line
    /// instead of being dropped. Whole-cell metrics plus a strip too
    /// narrow for the three labels, so both unit regimes overflow.
    #[test]
    fn a_wrap_descriptor_lays_out_a_second_line() {
        let horizontal = super::super::ToolbarOrientation::Horizontal;
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let narrow = Rect::new(0.0, 0.0, 24.0, 1.0);
        let clipped = Primitive::layout(&crowded_bar(), &ctx, narrow);
        let wrapped = Primitive::layout(
            &crowded_bar().with_overflow(super::super::ToolbarOverflow::Wrap),
            &ctx,
            narrow,
        );
        assert!(
            wrapped.layout.cross_extent(horizontal) > clipped.layout.cross_extent(horizontal),
            "a wrapped bar reports more cross-axis room than a clipped one: {} vs {}",
            wrapped.layout.cross_extent(horizontal),
            clipped.layout.cross_extent(horizontal),
        );
    }

    /// `Toolbar::orientation` reaches `Primitive::layout` too: a vertical
    /// bar stacks its items along the cross axis instead of the main one.
    #[test]
    fn a_vertical_descriptor_stacks_items_through_the_contract() {
        let vertical = crowded_bar().with_orientation(super::super::ToolbarOrientation::Vertical);
        for layout in layouts(&vertical) {
            let ys: Vec<f32> = layout
                .layout
                .visible_items
                .iter()
                .map(|v| v.bounds.y)
                .collect();
            assert!(
                ys.windows(2).all(|w| w[1] > w[0]),
                "every item starts below the one before it: {ys:?}"
            );
        }
    }

    /// `Toolbar::valign` lands on the resolved layout, which is what both
    /// painters read it from.
    #[test]
    fn the_descriptor_valign_reaches_the_resolved_layout() {
        let bottom = bar().with_valign(ToolbarVAlign::Bottom);
        for layout in layouts(&bottom) {
            assert_eq!(layout.valign, ToolbarVAlign::Bottom);
        }
        for layout in layouts(&bar()) {
            assert_eq!(layout.valign, ToolbarVAlign::Top);
        }
    }

    /// The options a host used to pass per call round-trip through the
    /// descriptor, so a caller holding a `ToolbarLayoutOptions` can hand
    /// it over whole.
    #[test]
    fn layout_options_round_trip_through_the_descriptor() {
        let options = super::super::ToolbarLayoutOptions::new()
            .with_orientation(super::super::ToolbarOrientation::Vertical)
            .with_overflow(super::super::ToolbarOverflow::Menu)
            .with_pin_trailing_overflow_to_origin(true);
        assert_eq!(bar().with_layout_options(options).layout_options(), options);
    }
}
