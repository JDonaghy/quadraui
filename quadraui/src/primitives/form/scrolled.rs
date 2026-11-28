//! [`ScrolledForm`] — a [`Form`] with a vertical [`Scrollbar`] beside it.
//!
//! A form whose fields overflow the rect it is laid out in reserves a
//! scrollbar track on its right edge. The track paints through
//! [`Scrollbar`]'s own painter at the form's live scroll offset; a press
//! on the thumb starts a thumb drag, and a press on the bare track pages
//! by one viewport. Every other event goes to [`Form`]'s own
//! [`Interactive`], so fields, keys, the wheel and focus traversal behave
//! exactly as on a bare form. The widget also paints the keyboard ring
//! around the focused item of a multi-item row (`ButtonRow`,
//! `ToggleGroup`, `Toolbar`), which the bare form leaves to its host.
//!
//! The widget's id is its form's, so its [`FormEvent`]s arrive as
//! [`UiEvent::Form`] under that id, and its [`FormState`] is stored
//! under it.
//!
//! # Hosting
//!
//! An app on the runtime's routing engine pushes the widget as a
//! `Surface::Widget` and registers it in its frame tree. A host that
//! reads raw [`UiEvent`]s keeps the widget and its [`FormState`],
//! paints it through [`ScrolledForm::draw`] and resolves each event
//! through [`ScrolledForm::handle_event`]:
//!
//! ```rust,ignore
//! let form = ScrolledForm::new(self.build_form());
//! // AppLogic::render:
//! form.draw(&self.form_state, backend, rect);
//! // AppLogic::handle:
//! match form.handle_event(&mut self.form_state, &event, backend, rect) {
//!     Handled::Event(event) => { /* apply the FormEvent to app state */ }
//!     Handled::Consumed => { /* scrolled, or an edit in progress: redraw */ }
//!     Handled::Ignored => {}
//! }
//! ```
//!
//! Field values, the focused field and validation stay on the [`Form`]
//! the host rebuilds every frame. [`crate::validation::validate`],
//! [`crate::validation::mark_required`] and
//! [`crate::visibility::apply_visibility`] are plain functions the host
//! applies to that form before wrapping it.

use super::contract as form_contract;
use super::{Form, FormEvent, FormPrimitiveLayout, FormState};
use crate::a11y::{A11yChild, A11yInfo, A11yPrimitive};
use crate::backend::{Backend, DrawsPrimitives};
use crate::compose::widget_host;
use crate::compose::Handled;
use crate::event::{Point, Rect, UiEvent};
use crate::interactive::{EventCx, FocusSpec, Interactive, PointerKind, RoutedEvent, SubTarget};
use crate::primitive::{LayoutCtx, Primitive, Size, Units};
use crate::primitives::scrollbar::{
    scroll_by_clamped, PressZone, Scrollbar, ScrollbarLayout, ThumbDrag,
};
use crate::types::WidgetId;
use crate::MouseButton;

/// A [`Form`] that reserves a vertical scrollbar track when its fields
/// overflow — see the module doc.
#[derive(Debug, Clone, PartialEq)]
pub struct ScrolledForm {
    /// The form, rebuilt by the host every frame.
    pub form: Form,
}

/// [`ScrolledForm`]'s resolved layout.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct ScrolledFormLayout {
    /// The rect the widget was laid out in.
    pub area: Rect,
    /// The form, laid out in `area` less the scrollbar track.
    pub form: FormPrimitiveLayout,
    /// The scrollbar, present when the fields overflow the form's rect.
    pub scrollbar: Option<(Scrollbar, ScrollbarLayout)>,
    /// Rows of fields the form's rect shows.
    pub viewport_rows: usize,
}

impl ScrolledFormLayout {
    /// Where field `id` paints, in surface coordinates — the anchor a
    /// host paints a field's own popup at (an open `ComboBox`). `None`
    /// when `id` names no field or the field is scrolled out of view.
    pub fn field_rect(&self, id: &WidgetId) -> Option<Rect> {
        let area = self.form.area;
        self.form.visible_field(id).map(|f| {
            Rect::new(
                area.x + f.bounds.x,
                area.y + f.bounds.y,
                f.bounds.width,
                f.bounds.height,
            )
        })
    }
}

impl ScrolledForm {
    /// `form`, with a scrollbar when it overflows.
    pub fn new(form: Form) -> Self {
        Self { form }
    }

    /// Paint the widget at `rect` with `state` as its live state, for a
    /// host that keeps the state itself — see the module doc.
    pub fn draw(&self, state: &FormState, backend: &mut dyn Backend, rect: Rect) {
        crate::widget_state::lend_widget_state(self.form.id, state.clone(), || {
            backend.draw_dyn(rect, self, &());
        });
    }

    /// The widget's layout at `rect` with `state` as its live state, as
    /// [`Self::draw`] paints it.
    pub fn layout_at(
        &self,
        state: &FormState,
        backend: &dyn Backend,
        rect: Rect,
    ) -> ScrolledFormLayout {
        crate::widget_state::lend_widget_state(self.form.id, state.clone(), || {
            backend.layout_primitive(rect, self)
        })
        .1
    }

    /// Resolve one raw [`UiEvent`] for a host that paints the widget at
    /// `rect` through [`Self::draw`], through the widget's own
    /// [`Interactive`] against `state`. The widget is the host's whole
    /// world: every field can take keys, so a host with several panels
    /// forwards keys only while this one is active. A press outside
    /// `rect` is [`Handled::Ignored`] and ends any drag in flight.
    pub fn handle_event(
        &self,
        state: &mut FormState,
        event: &UiEvent,
        backend: &dyn Backend,
        rect: Rect,
    ) -> Handled<FormEvent> {
        if let UiEvent::MouseDown { position, .. } | UiEvent::DoubleClick { position, .. } = event {
            if !rect.contains(*position) {
                state.slider_drag = None;
                state.thumb_drag = None;
                return Handled::Ignored;
            }
        }
        let layout = self.layout_at(state, backend, rect);
        let subs = Interactive::sub_regions(self, &layout);
        let Some(routed) = widget_host::routed_event(event, rect, &subs) else {
            return Handled::Ignored;
        };
        let units = crate::backend::primitive_layout_ctx(backend).units();
        widget_host::drive(self, self.form.id, state, &routed, &layout, units).handled
    }

    /// The form as painted: the descriptor overlaid with `state`'s caret,
    /// scrub position, scroll and field focus.
    fn live(&self, state: Option<&FormState>, focused: bool) -> Form {
        self.form.live(state, focused).into_owned()
    }

    /// The on-screen rect of the keyboard-focused item inside the focused
    /// `ButtonRow` / `ToggleGroup` / `Toolbar` field of the painted
    /// `form`, or `None` while the form has no focus or no item focus.
    fn item_focus_ring(
        form: &Form,
        state: &FormState,
        form_layout: &FormPrimitiveLayout,
    ) -> Option<Rect> {
        if !form.has_focus {
            return None;
        }
        let (focused_id, idx, items) = form_contract::current_item_focus(form, state)?;
        let (item_id, _) = items.get(idx).copied()?;
        let vf = form_layout.layout.visible_fields.iter().find(|vf| {
            form.fields
                .get(vf.field_idx)
                .is_some_and(|f| f.id == focused_id)
        })?;
        let (_, item) = vf.item_bounds.iter().find(|(id, _)| *id == item_id)?;
        let area = form_layout.area;
        Some(Rect::new(
            area.x + item.x,
            area.y + item.y,
            item.width,
            item.height,
        ))
    }
}

/// A press at surface `y` on the scrollbar track: the thumb starts a
/// drag, the bare track pages by one viewport.
fn press_track(
    form: &Form,
    state: &mut FormState,
    layout: &ScrolledFormLayout,
    scrollbar: &Scrollbar,
    y: f32,
    cx: &mut EventCx,
) -> Handled<FormEvent> {
    let rows = layout.viewport_rows;
    let max = form.fields.len().saturating_sub(rows);
    if max == 0 {
        return Handled::Ignored;
    }
    let offset = state.live_scroll_offset(form);
    match scrollbar.press_zone(y) {
        PressZone::Thumb => {
            state.thumb_drag = Some(ThumbDrag::begin(
                y,
                offset as f32,
                scrollbar.travel(),
                max as f32,
            ));
            cx.capture_pointer();
        }
        PressZone::Before => {
            state.scroll_offset = Some(scroll_by_clamped(offset, -(rows as isize), max));
        }
        PressZone::After => {
            state.scroll_offset = Some(scroll_by_clamped(offset, rows as isize, max));
        }
    }
    cx.request_redraw();
    Handled::Consumed
}

/// A pointer move to surface `y` during a thumb drag.
fn drag_thumb(form: &Form, state: &mut FormState, drag: ThumbDrag, y: f32) -> Handled<FormEvent> {
    let Some(offset) = drag.offset_at(y) else {
        return Handled::Ignored;
    };
    let offset = offset.round() as usize;
    if offset == state.live_scroll_offset(form) {
        return Handled::Ignored;
    }
    state.scroll_offset = Some(offset);
    Handled::Consumed
}

/// The row pitch the scrollbar geometry is resolved in: one cell on a
/// cell grid, the backend's line height on a pixel surface.
fn line_height(ctx: &LayoutCtx) -> f32 {
    match ctx.units() {
        Units::Cells => 1.0,
        _ => ctx.editor_line_height(),
    }
}

fn row_height(lh: f32) -> f32 {
    (lh * 1.4).round()
}

fn track_width(lh: f32) -> f32 {
    (lh * 0.4).max(1.0).round()
}

/// Rows of `lh`-pitched fields a `rect`-tall viewport shows.
fn viewport_rows(lh: f32, rect: Rect) -> usize {
    let rh = row_height(lh);
    if lh <= 0.0 || rh <= 0.0 {
        return 0;
    }
    (rect.height / rh).floor() as usize
}

/// `rect` split into the form's rect and, when `field_count` fields
/// overflow it, the scrollbar track on its right edge.
fn split_rect(field_count: usize, lh: f32, rect: Rect) -> (Rect, Option<Rect>) {
    let tw = track_width(lh);
    if rect.width <= tw {
        return (rect, None);
    }
    let form_rect = Rect::new(rect.x, rect.y, rect.width - tw, rect.height);
    if field_count <= viewport_rows(lh, form_rect) {
        return (rect, None);
    }
    let track = Rect::new(rect.x + rect.width - tw, rect.y, tw, rect.height);
    (form_rect, Some(track))
}

/// The scrollbar for `field_count` fields scrolled to `scroll_offset`,
/// on `track`.
fn build_scrollbar(
    id: &WidgetId,
    field_count: usize,
    scroll_offset: usize,
    dragging: bool,
    lh: f32,
    track: Rect,
) -> Scrollbar {
    let total = field_count as f32;
    let rh = row_height(lh);
    let visible = if rh > 0.0 {
        (track.height / rh).floor()
    } else {
        total
    };
    let mut sb = Scrollbar::vertical(
        format!("{}-scrollbar", id.as_str()),
        track,
        scroll_offset as f32,
        total,
        visible,
        lh.max(1.0),
    );
    sb.dragging = dragging;
    sb
}

impl A11yPrimitive for ScrolledForm {
    fn a11y_info(&self) -> A11yInfo {
        self.form.a11y_info()
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        self.form.a11y_children()
    }
}

impl ScrolledForm {
    /// [`Primitive::layout`] for `live`, a form with its live state
    /// already overlaid, scrolled with `dragging` saying whether a thumb
    /// drag is in flight — for a composite that keeps the form's state
    /// itself ([`crate::compose::FormDialog`]).
    pub(crate) fn layout_live(
        live: &Form,
        dragging: bool,
        ctx: &LayoutCtx,
        rect: Rect,
    ) -> ScrolledFormLayout {
        let lh = line_height(ctx);
        let (form_rect, track) = split_rect(live.fields.len(), lh, rect);
        let form = Primitive::layout(live, ctx, form_rect);
        let scrollbar = track.map(|track| {
            let sb = build_scrollbar(
                &live.id,
                live.fields.len(),
                live.scroll_offset,
                dragging,
                lh,
                track,
            );
            let sb_layout = ScrollbarLayout::resolve(&sb, track);
            (sb, sb_layout)
        });
        ScrolledFormLayout {
            area: rect,
            form,
            scrollbar,
            viewport_rows: viewport_rows(lh, form_rect),
        }
    }

    /// [`Primitive::paint`] for `live` at `layout` — see
    /// [`Self::layout_live`].
    pub(crate) fn paint_live(
        live: &Form,
        state: &FormState,
        layout: &ScrolledFormLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        Primitive::paint(live, &layout.form, surface, env);
        if let Some(ring) = Self::item_focus_ring(live, state, &layout.form) {
            surface.surface_stroke_rect(ring, env.theme().accent_fg, env.style().focus_ring_width);
        }
        if let Some((sb, sb_layout)) = &layout.scrollbar {
            Primitive::paint(sb, sb_layout, surface, env);
        }
    }

    /// [`Primitive::paint_cells`] for `live` at `layout` — see
    /// [`Self::layout_live`].
    #[cfg(feature = "tui")]
    pub(crate) fn paint_cells_live(
        live: &Form,
        state: &FormState,
        layout: &ScrolledFormLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        Primitive::paint_cells(live, &layout.form, cells, env);
        if let Some(ring) = Self::item_focus_ring(live, state, &layout.form) {
            crate::tui::draw_focus_ring(cells, ring, env.theme());
        }
        if let Some((sb, sb_layout)) = &layout.scrollbar {
            Primitive::paint_cells(sb, sb_layout, cells, env);
        }
    }

    /// Forward `event` to the form at `layout`, unless it is a press on
    /// the scrollbar track or a move or release of a thumb drag — for a
    /// composite that keeps the form's state itself, the same way
    /// [`Interactive::handle`] does.
    pub(crate) fn handle_live(
        live: &Form,
        state: &mut FormState,
        event: &RoutedEvent,
        layout: &ScrolledFormLayout,
        cx: &mut EventCx,
    ) -> Handled<FormEvent> {
        let origin = layout.area;
        let at = |local: &Point| Point::new(origin.x + local.x, origin.y + local.y);
        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                local,
                ..
            } => {
                state.slider_drag = None;
                state.thumb_drag = None;
                let p = at(local);
                if let Some((sb, _)) = layout
                    .scrollbar
                    .as_ref()
                    .filter(|(sb, _)| sb.track.contains(p))
                {
                    return press_track(live, state, layout, sb, p.y, cx);
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Move,
                local,
                ..
            } => {
                if let (Some(drag), None) = (state.thumb_drag, state.slider_drag.as_ref()) {
                    return drag_thumb(live, state, drag, at(local).y);
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Up(MouseButton::Left),
                ..
            } if state.thumb_drag.take().is_some() => {
                cx.release_pointer();
                return Handled::Consumed;
            }
            _ => {}
        }
        Interactive::handle(live, state, event, &layout.form, cx)
    }
}

impl Primitive for ScrolledForm {
    type Layout = ScrolledFormLayout;
    type Event = FormEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.form.id)
    }

    fn measure(&self, ctx: &LayoutCtx, avail: Size) -> crate::NaturalSize {
        Primitive::measure(&self.form, ctx, avail)
    }

    /// The form in `rect` less the scrollbar track its fields need, at
    /// the live state stored for this widget's id.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> ScrolledFormLayout {
        let stored = crate::widget_state::with_widget_state_ref(Some(&self.form.id), |state| {
            state.and_then(|s| s.downcast_ref::<FormState>()).cloned()
        });
        let live = self.live(stored.as_ref(), ctx.paint_state().focus_ring_visible());
        let dragging = stored.as_ref().is_some_and(|s| s.thumb_drag.is_some());
        Self::layout_live(&live, dragging, ctx, rect)
    }

    fn paint(
        &self,
        layout: &ScrolledFormLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let state = env.widget_state_as::<FormState>();
        let live = self.live(state, env.state.focus_ring_visible());
        let default = FormState::default();
        Self::paint_live(&live, state.unwrap_or(&default), layout, surface, env);
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &ScrolledFormLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let state = env.widget_state_as::<FormState>();
        let live = self.live(state, env.state.focus_ring_visible());
        let default = FormState::default();
        Self::paint_cells_live(&live, state.unwrap_or(&default), layout, cells, env);
    }
}

impl Interactive for ScrolledForm {
    type State = FormState;
    type Action = FormEvent;

    /// The form's own stops.
    fn focus(&self) -> FocusSpec {
        Interactive::focus(&self.form)
    }

    /// The form's own sub-regions: the form's rect starts at this
    /// widget's origin, so they need no translation.
    fn sub_regions(&self, layout: &ScrolledFormLayout) -> Vec<(Rect, SubTarget)> {
        Interactive::sub_regions(&self.form, &layout.form)
    }

    /// A press on the scrollbar track and every move of a thumb drag stay
    /// here; everything else goes to the form.
    fn handle(
        &self,
        state: &mut FormState,
        event: &RoutedEvent,
        layout: &ScrolledFormLayout,
        cx: &mut EventCx,
    ) -> Handled<FormEvent> {
        Self::handle_live(&self.form, state, event, layout, cx)
    }
}

#[cfg(test)]
#[path = "scrolled_tests.rs"]
mod tests;
