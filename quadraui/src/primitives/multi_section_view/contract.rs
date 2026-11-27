//! The widget contract for [`MultiSectionView`]: [`Primitive`] (layout
//! through [`MultiSectionView::resolve_layout`], paint through this
//! module's chrome painters plus each body's own widget) and
//! [`Interactive`] (headers, aux inputs, scrollbars and dividers handled
//! here; everything that lands on a body forwarded to that body widget's
//! own `handle`).

use std::any::Any;

use super::{
    AuxHit, HeaderHit, MultiSectionView, MultiSectionViewEvent, MultiSectionViewHit,
    MultiSectionViewPrimitiveLayout, ScrollMode, ScrollbarHit, SectionAux, SectionBody,
    SectionBodyLayout, SectionBodyView,
};
use crate::backend::{PointerShape, ResizeEdge};
use crate::behaviour::text_edit::{handle_flat_edit_event, FlatEditOutcome, FlatEditState};
use crate::event::{Key, MouseButton, NamedKey, Point, Rect};
use crate::interactive::{
    EventCx, EventHost, FocusSpec, FocusStop, Interactive, PointerKind, RoutedEvent, SubTarget,
    WidgetAction,
};
use crate::primitive::{LayoutCtx, Primitive, Units};
use crate::primitives::chart::ChartState;
use crate::primitives::form::FormState;
use crate::primitives::list::ListViewState;
use crate::primitives::scrollbar::{wheel_delta, wheel_row_delta, ThumbDrag};
use crate::primitives::terminal::TerminalState;
use crate::primitives::tree::TreeViewState;
use crate::types::WidgetId;
use crate::{Handled, InteractionState, PaintEnv};

// ── Sub-parts ──────────────────────────────────────────────────────────────

const KIND_SHIFT: u32 = 56;
const SECTION_SHIFT: u32 = 40;
const SECTION_MASK: u64 = 0xFFFF;
const CHILD_MASK: u64 = (1 << SECTION_SHIFT) - 1;

/// An addressable part of a [`MultiSectionView`], as named by its
/// [`SubTarget`]s: the focus stops [`Interactive::focus`] lists and the
/// regions [`Interactive::sub_regions`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiSectionViewSubPart {
    /// Section `.0`'s header row.
    Header(usize),
    /// Section `.0`'s aux row.
    Aux(usize),
    /// Section `.0`'s body, at the body widget's own sub-part `.1`
    /// (`None` for the body as a whole).
    Body(usize, Option<SubTarget>),
    /// Section `.0`'s scrollbar gutter.
    Scrollbar(usize),
    /// The divider below section `.0`.
    Divider(usize),
    /// The panel-level scrollbar.
    PanelScrollbar,
}

impl MultiSectionViewSubPart {
    /// The [`SubTarget`] naming this part. A body sub-part keeps the low
    /// 40 bits of the body widget's own target.
    pub fn sub_target(self) -> SubTarget {
        let pack = |kind: u64, section: usize, child: u64| {
            SubTarget(
                (kind << KIND_SHIFT)
                    | (((section as u64) & SECTION_MASK) << SECTION_SHIFT)
                    | (child & CHILD_MASK),
            )
        };
        match self {
            Self::Header(s) => pack(1, s, 0),
            Self::Aux(s) => pack(2, s, 0),
            Self::Body(s, None) => pack(3, s, CHILD_MASK),
            Self::Body(s, Some(child)) => pack(3, s, child.0),
            Self::Scrollbar(s) => pack(4, s, 0),
            Self::Divider(s) => pack(5, s, 0),
            Self::PanelScrollbar => pack(6, 0, 0),
        }
    }

    /// The part `sub` names, if it is one of this widget's.
    pub fn from_sub_target(sub: SubTarget) -> Option<Self> {
        let section = ((sub.0 >> SECTION_SHIFT) & SECTION_MASK) as usize;
        let child = sub.0 & CHILD_MASK;
        Some(match sub.0 >> KIND_SHIFT {
            1 => Self::Header(section),
            2 => Self::Aux(section),
            3 if child == CHILD_MASK => Self::Body(section, None),
            3 => Self::Body(section, Some(SubTarget(child))),
            4 => Self::Scrollbar(section),
            5 => Self::Divider(section),
            6 => Self::PanelScrollbar,
            _ => return None,
        })
    }
}

// ── State ──────────────────────────────────────────────────────────────────

/// One section body's own [`Interactive::State`], kept for the body
/// widget between events.
#[derive(Debug, Default)]
#[non_exhaustive]
pub enum SectionBodyState {
    /// No state yet, or a body with none of its own.
    #[default]
    None,
    Tree(TreeViewState),
    List(ListViewState),
    Form(FormState),
    Terminal(TerminalState),
    Chart(ChartState),
}

impl SectionBodyState {
    fn fresh(body: &SectionBody) -> Self {
        match body {
            SectionBody::Tree(_) => Self::Tree(TreeViewState::default()),
            SectionBody::List(_) => Self::List(ListViewState::default()),
            SectionBody::Form(_) => Self::Form(FormState::default()),
            SectionBody::Terminal(_) => Self::Terminal(TerminalState::default()),
            SectionBody::Chart(_) => Self::Chart(ChartState::default()),
            _ => Self::None,
        }
    }

    fn matches(&self, body: &SectionBody) -> bool {
        matches!(
            (self, body),
            (Self::Tree(_), SectionBody::Tree(_))
                | (Self::List(_), SectionBody::List(_))
                | (Self::Form(_), SectionBody::Form(_))
                | (Self::Terminal(_), SectionBody::Terminal(_))
                | (Self::Chart(_), SectionBody::Chart(_))
                | (
                    Self::None,
                    SectionBody::MessageList(_)
                        | SectionBody::Text(_)
                        | SectionBody::Empty(_)
                        | SectionBody::Custom(_)
                )
        )
    }

    /// The body widget's state, type-erased the way
    /// [`PaintEnv::widget_state`] carries it.
    fn as_any(&self) -> Option<&dyn Any> {
        match self {
            Self::None => None,
            Self::Tree(s) => Some(s),
            Self::List(s) => Some(s),
            Self::Form(s) => Some(s),
            Self::Terminal(s) => Some(s),
            Self::Chart(s) => Some(s),
        }
    }
}

/// A pointer gesture the view captured the pointer for.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    /// A divider drag between `above` and `below`, from pointer `origin`
    /// with both sections at their resolved sizes.
    Divider {
        above: usize,
        below: usize,
        origin: f32,
        above_size: f32,
        below_size: f32,
    },
    /// A section scrollbar thumb drag, in body rows.
    SectionThumb { section: usize, drag: ThumbDrag },
    /// A panel scrollbar thumb drag, in main-axis units.
    PanelThumb(ThumbDrag),
    /// A gesture the section's body widget captured the pointer for.
    Body(usize),
    /// A drag-select in the section's aux input.
    Aux(usize),
}

/// [`Interactive::State`] for [`MultiSectionView`]: the gesture in flight,
/// the focused part, each aux input's editing state and each body
/// widget's own state. Collapse, scroll and sizes are descriptor fields.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct MultiSectionViewState {
    drag: Option<Drag>,
    focused: Option<MultiSectionViewSubPart>,
    aux: Vec<FlatEditState>,
    bodies: Vec<SectionBodyState>,
}

impl MultiSectionViewState {
    /// Section `idx`'s body widget state, once the view has handled an
    /// event.
    pub fn body(&self, idx: usize) -> Option<&SectionBodyState> {
        self.bodies.get(idx)
    }

    /// Whether a divider, thumb or body gesture holds the pointer.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// One body state per section, reset where a section's body kind
    /// changed since the last event.
    fn sync(&mut self, view: &MultiSectionView) {
        let n = view.sections.len();
        self.bodies.resize_with(n, SectionBodyState::default);
        self.aux.resize_with(n, FlatEditState::default);
        for (state, section) in self.bodies.iter_mut().zip(&view.sections) {
            if !state.matches(&section.body) {
                *state = SectionBodyState::fresh(&section.body);
            }
        }
    }
}

// ── Body dispatch ──────────────────────────────────────────────────────────

/// Run `$e` with `$w` bound to the body widget, `$l` to its layout and
/// `$s` to its state (`&mut ()` for a stateless `MessageList`), for the
/// body kinds that are widgets; `$none` otherwise.
macro_rules! with_body_widget {
    ($body:expr, $layout:expr, $state:expr, |$w:ident, $l:ident, $s:ident| $e:expr, $none:expr) => {
        match ($body, $layout, $state) {
            (SectionBody::Tree($w), SectionBodyLayout::Tree($l), SectionBodyState::Tree($s)) => $e,
            (SectionBody::List($w), SectionBodyLayout::List($l), SectionBodyState::List($s)) => $e,
            (SectionBody::Form($w), SectionBodyLayout::Form($l), SectionBodyState::Form($s)) => $e,
            (
                SectionBody::Terminal($w),
                SectionBodyLayout::Terminal($l),
                SectionBodyState::Terminal($s),
            ) => $e,
            (SectionBody::Chart($w), SectionBodyLayout::Chart($l), SectionBodyState::Chart($s)) => {
                $e
            }
            (SectionBody::MessageList($w), SectionBodyLayout::MessageList($l), _) => {
                let $s = &mut ();
                $e
            }
            _ => $none,
        }
    };
}

/// Run `$e` with `$w` bound to a body widget and `$l` to its layout.
macro_rules! with_body_paint {
    ($body:expr, $layout:expr, |$w:ident, $l:ident| $e:expr) => {
        match ($body, $layout) {
            (SectionBody::Tree($w), SectionBodyLayout::Tree($l)) => $e,
            (SectionBody::List($w), SectionBodyLayout::List($l)) => $e,
            (SectionBody::Form($w), SectionBodyLayout::Form($l)) => $e,
            (SectionBody::Terminal($w), SectionBodyLayout::Terminal($l)) => $e,
            (SectionBody::MessageList($w), SectionBodyLayout::MessageList($l)) => $e,
            (SectionBody::Chart($w), SectionBodyLayout::Chart($l)) => $e,
            _ => {}
        }
    };
}
pub(crate) use with_body_paint;

/// The id a body widget carries, for the body kinds that are widgets.
fn body_id(body: &SectionBody) -> Option<WidgetId> {
    match body {
        SectionBody::Tree(w) => Some(w.id),
        SectionBody::List(w) => Some(w.id),
        SectionBody::Form(w) => Some(w.id),
        SectionBody::Terminal(w) => Some(w.id),
        SectionBody::MessageList(w) => Some(w.id),
        SectionBody::Chart(w) => Some(w.id),
        _ => None,
    }
}

impl MultiSectionView {
    /// The env section `idx`'s body paints with: hover and press on the
    /// body's own sub-parts re-addressed from this view to the body
    /// widget, the body widget's own state, and no per-call paint state.
    pub(crate) fn body_env<'a>(
        &self,
        idx: usize,
        env: &PaintEnv<'a>,
        interaction: &'a InteractionState,
    ) -> PaintEnv<'a> {
        let state = env
            .widget_state_as::<MultiSectionViewState>()
            .and_then(|s| s.bodies.get(idx))
            .and_then(SectionBodyState::as_any);
        env.with_state(&())
            .with_interaction(interaction)
            .with_widget_state(state)
    }

    /// Hover and press on section `idx`'s body, re-addressed from this
    /// view to the body widget.
    pub(crate) fn body_interaction(&self, idx: usize, env: &PaintEnv) -> InteractionState {
        let Some(child) = body_id(&self.sections[idx].body) else {
            return InteractionState::new();
        };
        let readdress = |target: Option<crate::focus::FocusAddress>| match target {
            Some((id, Some(sub))) if id == self.id => {
                match MultiSectionViewSubPart::from_sub_target(sub) {
                    Some(MultiSectionViewSubPart::Body(s, child_sub)) if s == idx => {
                        Some((child, child_sub))
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        InteractionState::from_targets(
            readdress(env.interaction.hovered_target()),
            readdress(env.interaction.pressed_target()),
        )
    }
}

// ── Primitive ──────────────────────────────────────────────────────────────

impl Primitive for MultiSectionView {
    type Layout = MultiSectionViewPrimitiveLayout;
    type Event = MultiSectionViewEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// Round-trips through [`crate::frame::OwnedContent::MultiSectionView`].
    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::MultiSectionView(self.clone())
    }

    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> Self::Layout {
        self.resolve_layout(ctx, rect)
    }

    fn paint(
        &self,
        layout: &Self::Layout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &PaintEnv,
    ) {
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        super::native_surface_paint::paint(self, layout, surface, env, true);
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        let _ = (layout, surface, env);
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &Self::Layout,
        cells: &mut dyn crate::CellSurface,
        env: &PaintEnv,
    ) {
        super::paint_cells::paint(self, layout, cells, env);
    }
}

// ── Interactive ────────────────────────────────────────────────────────────

/// The [`EventHost`] a section's body widget or aux input handles events
/// against: focus is re-addressed to that part of this view, and a
/// pointer capture or release is recorded so the view keeps routing the
/// gesture to the part.
struct PartHost<'a, 'b> {
    parent: &'a mut EventCx<'b>,
    section: usize,
    aux: bool,
    captured: Option<bool>,
}

impl PartHost<'_, '_> {
    fn part(&self, sub: Option<SubTarget>) -> Option<SubTarget> {
        let part = if self.aux {
            MultiSectionViewSubPart::Aux(self.section)
        } else {
            MultiSectionViewSubPart::Body(self.section, sub)
        };
        Some(part.sub_target())
    }
}

impl EventHost for PartHost<'_, '_> {
    fn is_focused(&self, _id: &WidgetId, sub: Option<SubTarget>) -> bool {
        self.parent.is_focused(self.part(sub))
    }
    fn request_focus(&mut self, _id: &WidgetId, sub: Option<SubTarget>) {
        let part = self.part(sub);
        self.parent.request_focus(part);
    }
    fn capture_pointer(&mut self, _id: &WidgetId) {
        self.parent.capture_pointer();
        self.captured = Some(true);
    }
    fn release_pointer(&mut self) {
        self.parent.release_pointer();
        self.captured = Some(false);
    }
    fn set_cursor(&mut self, cursor: PointerShape) {
        self.parent.set_cursor(cursor);
    }
    fn request_redraw(&mut self) {
        self.parent.request_redraw();
    }
    fn emit(&mut self, event: crate::UiEvent) {
        self.parent.emit(event);
    }
    fn units(&self) -> Units {
        self.parent.units()
    }
}

fn origin(layout: &MultiSectionViewPrimitiveLayout) -> Point {
    Point::new(layout.chrome.bounds.x, layout.chrome.bounds.y)
}

/// `rect` moved into the view's local space.
fn local_rect(layout: &MultiSectionViewPrimitiveLayout, rect: Rect) -> Rect {
    let o = origin(layout);
    Rect::new(rect.x - o.x, rect.y - o.y, rect.width, rect.height)
}

/// `event` with its pointer position moved from the view's local space to
/// the body's (`delta` is the body rect's offset inside the view).
fn reroute(event: &RoutedEvent, delta: Point) -> RoutedEvent {
    let shift = |p: &Point| Point::new(p.x - delta.x, p.y - delta.y);
    match event {
        RoutedEvent::Pointer {
            kind,
            local,
            mods,
            sub,
        } => RoutedEvent::Pointer {
            kind: *kind,
            sub: sub.and_then(|s| match MultiSectionViewSubPart::from_sub_target(s) {
                Some(MultiSectionViewSubPart::Body(_, child)) => child,
                _ => None,
            }),
            local: shift(local),
            mods: *mods,
        },
        RoutedEvent::Wheel {
            dx,
            dy,
            local,
            mods,
        } => RoutedEvent::Wheel {
            dx: *dx,
            dy: *dy,
            local: shift(local),
            mods: *mods,
        },
        other => other.clone(),
    }
}

/// The char index of byte offset `byte` in `text`.
fn char_index(text: &str, byte: usize) -> usize {
    text.char_indices().take_while(|(i, _)| *i < byte).count()
}

/// The byte offset of char index `chars` in `text`.
fn byte_index(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(i, _)| i)
}

impl MultiSectionView {
    /// Forward `event` (in the view's local space) to section `idx`'s
    /// body widget, wrapping what it reports as
    /// [`MultiSectionViewEvent::Body`].
    fn forward_to_body(
        &self,
        idx: usize,
        state: &mut MultiSectionViewState,
        event: &RoutedEvent,
        layout: &MultiSectionViewPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<MultiSectionViewEvent> {
        let Some(view) = layout.body(idx) else {
            return Handled::Ignored;
        };
        let Some(section) = self.sections.get(idx) else {
            return Handled::Ignored;
        };
        let o = origin(layout);
        let routed = reroute(event, Point::new(view.rect.x - o.x, view.rect.y - o.y));
        let Some(child_state) = state.bodies.get_mut(idx) else {
            return Handled::Ignored;
        };
        let mut host = PartHost {
            parent: cx,
            section: idx,
            aux: false,
            captured: None,
        };
        let handled = with_body_widget!(
            &section.body,
            &view.layout,
            child_state,
            |w, l, s| {
                let mut child_cx = EventCx::new(w.id, &mut host);
                Interactive::handle(w, s, &routed, l, &mut child_cx).map(|action| {
                    MultiSectionViewEvent::Body(idx, Box::new(action.into_ui_event(w.id)))
                })
            },
            Handled::Ignored
        );
        match host.captured {
            Some(true) => state.drag = Some(Drag::Body(idx)),
            Some(false) if state.drag == Some(Drag::Body(idx)) => state.drag = None,
            _ => {}
        }
        handled
    }

    /// The row count, visible rows and current offset of section `idx`'s
    /// row-scrolled (tree or list) body.
    fn body_rows(
        &self,
        idx: usize,
        layout: &MultiSectionViewPrimitiveLayout,
    ) -> Option<(usize, usize, usize)> {
        let section = self.sections.get(idx)?;
        let (offset, rows) = match &section.body {
            SectionBody::Tree(t) => (t.scroll_offset, t.rows.len()),
            SectionBody::List(l) => (
                l.scroll_offset,
                l.items.len() + usize::from(l.title.is_some()),
            ),
            _ => return None,
        };
        let body = layout.section(idx)?.body_bounds;
        let content =
            super::msv_body_measure(&section.body, &section.aux, layout.line_height as f64)
                .content_size;
        if rows == 0 || content <= 0.0 {
            return None;
        }
        let pitch = content / rows as f32;
        let visible = (body.height / pitch).floor().max(1.0) as usize;
        Some((rows, visible, offset))
    }

    /// Total main-axis content height of the panel and the largest panel
    /// scroll it allows.
    fn panel_extent(&self, layout: &MultiSectionViewPrimitiveLayout) -> (f32, f32) {
        let chrome = &layout.chrome;
        let total: f32 = chrome.sections.iter().map(|s| s.resolved_size).sum::<f32>()
            + chrome.dividers.iter().map(|d| d.bounds.height).sum::<f32>();
        (total, (total - chrome.bounds.height).max(0.0))
    }

    fn scroll_section_to(
        &self,
        idx: usize,
        offset: usize,
        current: usize,
    ) -> Handled<MultiSectionViewEvent> {
        if offset == current {
            return Handled::Consumed;
        }
        Handled::Event(MultiSectionViewEvent::ScrollChanged {
            section: Some(idx),
            offset: offset as f32,
        })
    }

    fn scroll_panel_to(&self, offset: f32) -> Handled<MultiSectionViewEvent> {
        if (offset - self.panel_scroll).abs() < f32::EPSILON {
            return Handled::Consumed;
        }
        Handled::Event(MultiSectionViewEvent::ScrollChanged {
            section: None,
            offset,
        })
    }

    /// A header press or keyboard activation: the header's own event,
    /// plus a collapse toggle for its title area or chevron when the view
    /// allows collapsing.
    fn activate_header(
        &self,
        idx: usize,
        hit: HeaderHit,
        cx: &mut EventCx,
    ) -> Handled<MultiSectionViewEvent> {
        let toggles = self.allow_collapse && !matches!(hit, HeaderHit::Action(_));
        if toggles {
            let collapsed = !self.sections[idx].collapsed;
            cx.emit(
                MultiSectionViewEvent::CollapseChanged {
                    section: idx,
                    collapsed,
                }
                .into_ui_event(self.id),
            );
        }
        Handled::Event(MultiSectionViewEvent::HeaderActivated { section: idx, hit })
    }

    fn press(
        &self,
        state: &mut MultiSectionViewState,
        event: &RoutedEvent,
        abs: Point,
        layout: &MultiSectionViewPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<MultiSectionViewEvent> {
        match layout.hit_test(abs.x, abs.y) {
            MultiSectionViewHit::Header { section, kind } => {
                cx.request_focus(Some(MultiSectionViewSubPart::Header(section).sub_target()));
                self.activate_header(section, kind, cx)
            }
            MultiSectionViewHit::Aux { section, kind } => match kind {
                AuxHit::Input => self.edit_aux(section, state, event, layout, cx),
                hit => Handled::Event(MultiSectionViewEvent::AuxActivated { section, hit }),
            },
            MultiSectionViewHit::Body { section } => {
                self.forward_to_body(section, state, event, layout, cx)
            }
            MultiSectionViewHit::Divider { above, below } => {
                let size = |i: usize| layout.section(i).map_or(0.0, |s| s.resolved_size);
                state.drag = Some(Drag::Divider {
                    above,
                    below,
                    origin: abs.y,
                    above_size: size(above),
                    below_size: size(below),
                });
                cx.capture_pointer();
                cx.set_cursor(PointerShape::Resize(ResizeEdge::South));
                Handled::Consumed
            }
            MultiSectionViewHit::Scrollbar { section, kind } => {
                let Some((rows, visible, offset)) = self.body_rows(section, layout) else {
                    return Handled::Consumed;
                };
                let max = rows.saturating_sub(visible);
                match kind {
                    ScrollbarHit::Thumb => {
                        let s = layout.section(section);
                        let gutter = s.and_then(|s| s.scrollbar_bounds);
                        let thumb = s.and_then(|s| s.thumb_bounds).or(gutter);
                        let travel = match (gutter, thumb) {
                            (Some(g), Some(t)) => (g.height - t.height).max(0.0),
                            _ => 0.0,
                        };
                        state.drag = Some(Drag::SectionThumb {
                            section,
                            drag: ThumbDrag::begin(abs.y, offset as f32, travel, max as f32),
                        });
                        cx.capture_pointer();
                        Handled::Consumed
                    }
                    ScrollbarHit::TrackBefore => {
                        self.scroll_section_to(section, offset.saturating_sub(visible), offset)
                    }
                    ScrollbarHit::TrackAfter => {
                        self.scroll_section_to(section, (offset + visible).min(max), offset)
                    }
                }
            }
            MultiSectionViewHit::PanelScrollbar { kind } => {
                let (_, max) = self.panel_extent(layout);
                let page = layout.chrome.bounds.height;
                match kind {
                    ScrollbarHit::Thumb => {
                        let travel = layout
                            .chrome
                            .panel_scrollbar_thumb
                            .map_or(0.0, |t| (page - t.height).max(0.0));
                        state.drag = Some(Drag::PanelThumb(ThumbDrag::begin(
                            abs.y,
                            self.panel_scroll,
                            travel,
                            max,
                        )));
                        cx.capture_pointer();
                        Handled::Consumed
                    }
                    ScrollbarHit::TrackBefore => {
                        self.scroll_panel_to((self.panel_scroll - page).clamp(0.0, max))
                    }
                    ScrollbarHit::TrackAfter => {
                        self.scroll_panel_to((self.panel_scroll + page).clamp(0.0, max))
                    }
                }
            }
            MultiSectionViewHit::Inert | MultiSectionViewHit::Outside => Handled::Ignored,
        }
    }

    /// A pointer move or release while the view holds `drag`.
    fn drag_to(
        &self,
        drag: Drag,
        state: &mut MultiSectionViewState,
        event: &RoutedEvent,
        abs: Point,
        layout: &MultiSectionViewPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<MultiSectionViewEvent> {
        let released = matches!(
            event,
            RoutedEvent::Pointer {
                kind: PointerKind::Up(_),
                ..
            }
        );
        match drag {
            Drag::Body(idx) => return self.forward_to_body(idx, state, event, layout, cx),
            Drag::Aux(idx) => return self.edit_aux(idx, state, event, layout, cx),
            _ => {}
        }
        if released {
            state.drag = None;
            cx.release_pointer();
            return Handled::Consumed;
        }
        match drag {
            Drag::Divider {
                above,
                below,
                origin,
                above_size,
                below_size,
            } => {
                let mut resized = self.clone();
                if !resized.resize_divider(above, below, above_size, below_size, abs.y - origin) {
                    return Handled::Consumed;
                }
                let fixed = |i: usize| match resized.sections[i].size {
                    super::SectionSize::Fixed(v) => v,
                    _ => 0,
                };
                Handled::Event(MultiSectionViewEvent::SectionResized {
                    above,
                    above_size: fixed(above),
                    below,
                    below_size: fixed(below),
                })
            }
            Drag::SectionThumb { section, drag } => {
                let Some((_, _, current)) = self.body_rows(section, layout) else {
                    return Handled::Consumed;
                };
                match drag.offset_at(abs.y) {
                    Some(offset) => {
                        self.scroll_section_to(section, offset.round() as usize, current)
                    }
                    None => Handled::Consumed,
                }
            }
            Drag::PanelThumb(drag) => match drag.offset_at(abs.y) {
                Some(offset) => self.scroll_panel_to(offset),
                None => Handled::Consumed,
            },
            Drag::Body(_) | Drag::Aux(_) => Handled::Ignored,
        }
    }

    /// Route an editing event to section `idx`'s aux input.
    fn edit_aux(
        &self,
        idx: usize,
        state: &mut MultiSectionViewState,
        event: &RoutedEvent,
        layout: &MultiSectionViewPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<MultiSectionViewEvent> {
        let Some(SectionAux::Input(input) | SectionAux::Search(input)) =
            self.sections.get(idx).and_then(|s| s.aux.as_ref())
        else {
            return Handled::Ignored;
        };
        if let RoutedEvent::Key(key) = event {
            if key.key == Key::Named(NamedKey::Enter) {
                return Handled::Event(MultiSectionViewEvent::AuxInputSubmitted {
                    section: idx,
                    text: input.text.clone(),
                });
            }
        }
        let Some(aux) = layout.section(idx).and_then(|s| s.aux_bounds) else {
            return Handled::Ignored;
        };
        let aux = local_rect(layout, aux);
        let (pad, advance) = match layout.units {
            Units::Cells => (0.0, 1.0),
            _ => (4.0, layout.char_width.max(1.0)),
        };
        let text = input.text.as_str();
        let offset_at = |p: Point| {
            if !aux.contains(p) {
                return None;
            }
            let col = ((p.x - aux.x - pad) / advance).round().max(0.0) as usize;
            Some(byte_index(text, col))
        };
        let Some(edit) = state.aux.get_mut(idx) else {
            return Handled::Ignored;
        };
        let cursor = Some(byte_index(text, input.caret));
        let mut host = PartHost {
            parent: cx,
            section: idx,
            aux: true,
            captured: None,
        };
        let outcome = {
            let mut aux_cx = EventCx::new(input.id, &mut host);
            handle_flat_edit_event(text, cursor, None, edit, event, &offset_at, &mut aux_cx)
        };
        match host.captured {
            Some(true) => state.drag = Some(Drag::Aux(idx)),
            Some(false) if state.drag == Some(Drag::Aux(idx)) => state.drag = None,
            _ => {}
        }
        let edit = &state.aux[idx];
        match outcome {
            FlatEditOutcome::Changed(new_text) => {
                let caret = char_index(&new_text, edit.cursor.unwrap_or(new_text.len()));
                Handled::Event(MultiSectionViewEvent::AuxInputChanged {
                    section: idx,
                    text: new_text,
                    caret,
                })
            }
            FlatEditOutcome::Consumed => {
                let caret = edit.cursor.map_or(input.caret, |c| char_index(text, c));
                if caret == input.caret {
                    Handled::Consumed
                } else {
                    Handled::Event(MultiSectionViewEvent::AuxInputChanged {
                        section: idx,
                        text: input.text.clone(),
                        caret,
                    })
                }
            }
            FlatEditOutcome::Ignored => Handled::Ignored,
        }
    }

    /// Route a keyboard event to the focused part.
    fn key_or_text(
        &self,
        state: &mut MultiSectionViewState,
        event: &RoutedEvent,
        layout: &MultiSectionViewPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<MultiSectionViewEvent> {
        let focused = self.focused_part(cx).or(state.focused);
        match focused {
            Some(MultiSectionViewSubPart::Header(idx)) if idx < self.sections.len() => {
                match event {
                    RoutedEvent::Key(key)
                        if matches!(key.key, Key::Named(NamedKey::Enter) | Key::Char(' ')) =>
                    {
                        self.activate_header(idx, HeaderHit::TitleArea, cx)
                    }
                    _ => Handled::Ignored,
                }
            }
            Some(MultiSectionViewSubPart::Aux(idx)) => self.edit_aux(idx, state, event, layout, cx),
            Some(MultiSectionViewSubPart::Body(idx, _)) => {
                self.forward_to_body(idx, state, event, layout, cx)
            }
            _ => Handled::Ignored,
        }
    }

    /// The focus stop the host reports focused, if any.
    fn focused_part(&self, cx: &EventCx) -> Option<MultiSectionViewSubPart> {
        let FocusSpec::Children(stops) = Interactive::focus(self) else {
            return None;
        };
        stops
            .into_iter()
            .find(|stop| cx.is_focused(Some(stop.sub)))
            .and_then(|stop| MultiSectionViewSubPart::from_sub_target(stop.sub))
    }

    /// Move focus to `part`, telling the part that held it and the part
    /// that takes it.
    fn focus_moved(
        &self,
        state: &mut MultiSectionViewState,
        part: Option<MultiSectionViewSubPart>,
        layout: &MultiSectionViewPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<MultiSectionViewEvent> {
        let previous = std::mem::replace(&mut state.focused, part);
        match previous {
            Some(MultiSectionViewSubPart::Body(idx, _)) if !matches!(part, Some(MultiSectionViewSubPart::Body(i, _)) if i == idx) =>
            {
                let _ = self.forward_to_body(idx, state, &RoutedEvent::FocusOut, layout, cx);
            }
            Some(MultiSectionViewSubPart::Aux(idx)) if part != previous => {
                let _ = self.edit_aux(idx, state, &RoutedEvent::FocusOut, layout, cx);
            }
            _ => {}
        }
        match part {
            Some(MultiSectionViewSubPart::Body(idx, child)) => {
                self.forward_to_body(idx, state, &RoutedEvent::FocusIn(child), layout, cx)
            }
            Some(MultiSectionViewSubPart::Aux(idx)) => {
                let _ = self.edit_aux(idx, state, &RoutedEvent::FocusIn(None), layout, cx);
                Handled::Consumed
            }
            _ => Handled::Consumed,
        }
    }
}

impl Interactive for MultiSectionView {
    type State = MultiSectionViewState;
    type Action = MultiSectionViewEvent;

    /// One stop per header, then each expanded section's aux input and its
    /// body widget's own stops, in section order.
    fn focus(&self) -> FocusSpec {
        let mut stops = Vec::new();
        for (idx, section) in self.sections.iter().enumerate() {
            let mut push = |part: MultiSectionViewSubPart| {
                stops.push(FocusStop {
                    sub: part.sub_target(),
                })
            };
            push(MultiSectionViewSubPart::Header(idx));
            if section.collapsed {
                continue;
            }
            if matches!(
                section.aux,
                Some(SectionAux::Input(_) | SectionAux::Search(_))
            ) {
                push(MultiSectionViewSubPart::Aux(idx));
            }
            let body_focus = match &section.body {
                SectionBody::Tree(w) => Interactive::focus(w),
                SectionBody::List(w) => Interactive::focus(w),
                SectionBody::Form(w) => Interactive::focus(w),
                SectionBody::Terminal(w) => Interactive::focus(w),
                SectionBody::MessageList(w) => Interactive::focus(w),
                SectionBody::Chart(w) => Interactive::focus(w),
                _ => FocusSpec::None,
            };
            match body_focus {
                FocusSpec::None => {}
                FocusSpec::Stop => push(MultiSectionViewSubPart::Body(idx, None)),
                FocusSpec::Children(children) => {
                    for child in children {
                        push(MultiSectionViewSubPart::Body(idx, Some(child.sub)));
                    }
                }
            }
        }
        if stops.is_empty() {
            FocusSpec::None
        } else {
            FocusSpec::Children(stops)
        }
    }

    /// Headers, aux rows, each body's own sub-parts and then the body as
    /// a whole, scrollbars, dividers and the panel scrollbar — in the
    /// view's local space.
    fn sub_regions(&self, layout: &Self::Layout) -> Vec<(Rect, SubTarget)> {
        let mut out = Vec::new();
        for s in &layout.chrome.sections {
            let idx = s.section_idx;
            out.push((
                local_rect(layout, s.header_bounds),
                MultiSectionViewSubPart::Header(idx).sub_target(),
            ));
            if let Some(aux) = s.aux_bounds {
                out.push((
                    local_rect(layout, aux),
                    MultiSectionViewSubPart::Aux(idx).sub_target(),
                ));
            }
            if let (Some(view), Some(section)) = (layout.body(idx), self.sections.get(idx)) {
                let body_regions: Vec<(Rect, SubTarget)> = with_body_regions(&section.body, view);
                let rect = local_rect(layout, view.rect);
                for (r, sub) in body_regions {
                    out.push((
                        Rect::new(r.x + rect.x, r.y + rect.y, r.width, r.height),
                        MultiSectionViewSubPart::Body(idx, Some(sub)).sub_target(),
                    ));
                }
                out.push((
                    local_rect(layout, view.clip),
                    MultiSectionViewSubPart::Body(idx, None).sub_target(),
                ));
            }
            if let Some(gutter) = s.scrollbar_bounds {
                out.push((
                    local_rect(layout, gutter),
                    MultiSectionViewSubPart::Scrollbar(idx).sub_target(),
                ));
            }
        }
        for d in &layout.chrome.dividers {
            out.push((
                local_rect(layout, d.bounds),
                MultiSectionViewSubPart::Divider(d.above).sub_target(),
            ));
        }
        if let Some(panel) = layout.chrome.panel_scrollbar {
            out.push((
                local_rect(layout, panel),
                MultiSectionViewSubPart::PanelScrollbar.sub_target(),
            ));
        }
        out
    }

    fn handle(
        &self,
        state: &mut MultiSectionViewState,
        event: &RoutedEvent,
        layout: &Self::Layout,
        cx: &mut EventCx,
    ) -> Handled<MultiSectionViewEvent> {
        state.sync(self);
        let o = origin(layout);
        match event {
            RoutedEvent::Pointer { kind, local, .. } => {
                let abs = Point::new(local.x + o.x, local.y + o.y);
                if let Some(drag) = state.drag {
                    match kind {
                        PointerKind::Move => {
                            return self.drag_to(drag, state, event, abs, layout, cx)
                        }
                        PointerKind::Up(_) => {
                            return self.drag_to(drag, state, event, abs, layout, cx)
                        }
                        _ => {}
                    }
                }
                match kind {
                    PointerKind::Down(MouseButton::Left) => {
                        self.press(state, event, abs, layout, cx)
                    }
                    PointerKind::Down(_) | PointerKind::DoubleClick(_) | PointerKind::Up(_) => {
                        match layout.hit_test(abs.x, abs.y) {
                            MultiSectionViewHit::Body { section } => {
                                self.forward_to_body(section, state, event, layout, cx)
                            }
                            _ => Handled::Ignored,
                        }
                    }
                    PointerKind::Move | PointerKind::Enter => match layout.hit_test(abs.x, abs.y) {
                        MultiSectionViewHit::Body { section } => {
                            self.forward_to_body(section, state, event, layout, cx)
                        }
                        _ => Handled::Ignored,
                    },
                    PointerKind::Leave => Handled::Ignored,
                }
            }
            RoutedEvent::Wheel { dy, local, .. } => {
                if self.scroll_mode == ScrollMode::WholePanel {
                    let (_, max) = self.panel_extent(layout);
                    let delta = wheel_delta(*dy, layout.line_height);
                    return self.scroll_panel_to((self.panel_scroll + delta).clamp(0.0, max));
                }
                let abs = Point::new(local.x + o.x, local.y + o.y);
                match layout.hit_test(abs.x, abs.y) {
                    MultiSectionViewHit::Body { section }
                    | MultiSectionViewHit::Scrollbar { section, .. } => {
                        let body = layout.body(section).map(|v| v.clip);
                        let on_body = body.is_some_and(|b| b.contains(abs));
                        if on_body {
                            return self.forward_to_body(section, state, event, layout, cx);
                        }
                        let Some((rows, visible, offset)) = self.body_rows(section, layout) else {
                            return Handled::Ignored;
                        };
                        let delta = wheel_row_delta(*dy, 1.0);
                        let max = rows.saturating_sub(visible) as isize;
                        let next = (offset as isize + delta).clamp(0, max) as usize;
                        self.scroll_section_to(section, next, offset)
                    }
                    _ => Handled::Ignored,
                }
            }
            RoutedEvent::Key(_) | RoutedEvent::Text(_) => {
                self.key_or_text(state, event, layout, cx)
            }
            RoutedEvent::FocusIn(sub) => {
                let part = sub
                    .and_then(MultiSectionViewSubPart::from_sub_target)
                    .or_else(|| {
                        self.active_section
                            .filter(|i| *i < self.sections.len())
                            .map(MultiSectionViewSubPart::Header)
                    });
                self.focus_moved(state, part, layout, cx)
            }
            RoutedEvent::FocusOut => self.focus_moved(state, None, layout, cx),
            _ => Handled::Ignored,
        }
    }
}

/// A body widget's own sub-regions, in its own local space.
fn with_body_regions(body: &SectionBody, view: &SectionBodyView) -> Vec<(Rect, SubTarget)> {
    match (body, &view.layout) {
        (SectionBody::Tree(w), SectionBodyLayout::Tree(l)) => Interactive::sub_regions(w, l),
        (SectionBody::List(w), SectionBodyLayout::List(l)) => Interactive::sub_regions(w, l),
        (SectionBody::Form(w), SectionBodyLayout::Form(l)) => Interactive::sub_regions(w, l),
        (SectionBody::Terminal(w), SectionBodyLayout::Terminal(l)) => {
            Interactive::sub_regions(w, l)
        }
        (SectionBody::MessageList(w), SectionBodyLayout::MessageList(l)) => {
            Interactive::sub_regions(w, l)
        }
        (SectionBody::Chart(w), SectionBodyLayout::Chart(l)) => Interactive::sub_regions(w, l),
        _ => Vec::new(),
    }
}
