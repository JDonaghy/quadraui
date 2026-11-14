//! The `event -> hit -> semantic` orchestration every binding's own
//! [`quadraui::AppLogic::handle`] would otherwise have to re-pour:
//! menu-bar/context-menu routing, divider/scrollbar drag arming and
//! translation, click-to-focus, focus-scoped key routing, and the raw ->
//! semantic event substitution [`crate::semantic::semantic_event_for_hit`]
//! performs. [`InteractionEngine`] owns every piece of state that
//! orchestration touches; a binding's own `AppLogic` impl holds one, feeds
//! it the frame's painted hits after each render, and forwards `handle` to
//! it with an [`EventSink`] that bridges back into its own user-code
//! callback — the one binding-specific step this engine cannot do itself.

use std::cell::{Ref, RefCell};
use std::collections::{HashMap, HashSet};

use quadraui::backend::MessageDialogOptions;
use quadraui::compose::{
    ContextMenuController, ContextMenuOutcome, FormController, FormControllerAction,
    FormDialogAction, FormDialogController, Handled, MenuDef, MenuEvent, MenuSystem,
    MessageDialogController,
};
use quadraui::primitives::form::focus_order;
use quadraui::primitives::scrollbar::{scroll_by_clamped, ThumbDrag};
use quadraui::{
    wheel_row_delta, Backend, CompletionItemMeasure, Completions, CompletionsHit,
    CompletionsLayout, ContextMenu, ContextMenuItem, ContextMenuPlacement, DataTableEvent, Dialog,
    EditOp, Float, FloatLayout, FloatMeasure, Form, FormEvent, InteractionState, Key, MenuBar,
    MessageDialogButton, Modifiers, NamedKey, Point, Rect, RichTextPopup, RichTextPopupHit,
    RichTextPopupLayout, RichTextPopupMeasure, ScrollAxis, ScrollViewEvent, TabBarEvent,
    TextDisplayEvent, TextInput, Toast, ToastCorner, ToastHit, ToastOverlay, ToastStackLayout,
    Tooltip, TooltipLayout, TooltipMeasure, TreeEvent, UiEvent, WidgetId,
};

use crate::drag::{
    enclosing_scroll_child, find_data_table_layout, find_split_layout, find_split_tree_layout,
    markdown_at, scroll_scrollbar, scroll_view_at, text_display_scroll_state, tree_scroll_state,
    ActiveDrag, DataTableColumnDrag, ScrollDrag, ScrollbarDrag,
};
use crate::extra_paint::extra_as;
use crate::hit::{
    chrome_hover_hit, data_table_rows_and_viewport, event_position, hit_test_painted,
    list_rows_and_viewport, set_event_widget, tree_rows_and_viewport, HitDetail, PaintedHit,
};
use crate::host::{
    apply_data_table_nav_key, apply_list_nav_key, apply_scroll_offset, apply_split_divider_drag,
    apply_tab_activated, apply_tab_closed, apply_tree_nav_key, apply_tree_toggle_expand,
};
use crate::layout::LayoutNode;
use crate::render::paint_layout;
use crate::semantic::{
    button_clicked_event, focus_scoped_key_event, number_input_click_event, semantic_event_for_hit,
};
use crate::text_input::{text_input_click_event, text_input_insert_event, TextEditorCache};
use crate::widget_state::WidgetState;

/// `backend.viewport()`, as a `(0, 0)`-origin [`Rect`] — the shape every
/// anchored-overlay layout call here (`Tooltip`/`Completions`/`Float`/
/// `RichTextPopup`/`ToastOverlay`) takes as its own viewport-clamp
/// argument.
fn viewport_rect(backend: &dyn Backend) -> Rect {
    let viewport = backend.viewport();
    Rect::new(0.0, 0.0, viewport.width, viewport.height)
}

/// An arbitrary [`LayoutNode`] subtree embedded in an overlay — a
/// popover's content ([`InteractionEngine::show_popover_layout`]) or a
/// modal dialog's ([`InteractionEngine::open_layout_dialog`]) — plus
/// everything the engine needs to paint it and route events into it, so
/// both overlay kinds share one implementation of "lay out, paint, focus,
/// dispatch" rather than each growing its own.
struct LayoutContent {
    /// The subtree itself, laid out and painted fresh every frame through
    /// [`crate::render::paint_layout`] — the same function a binding's own
    /// render-callback path uses for its main tree — instead of a bespoke
    /// per-overlay-kind renderer.
    node: LayoutNode,
    /// This engine's own record of what that call most recently painted,
    /// mirroring [`InteractionEngine::hit_map`] but scoped to this one
    /// overlay, read back by [`Self::dispatch`]/[`Self::tab_stops`] to
    /// hit-test and focus-cycle the subtree exactly like the main tree's
    /// own hit map already does.
    hits: RefCell<Vec<PaintedHit>>,
    /// Mirrors [`InteractionEngine::form_controllers`], scoped the same
    /// way — a `"form"` extra nested anywhere in the subtree gets its own
    /// [`FormController`], kept deliberately separate from the main tree's
    /// cache so closing the overlay can never drop a main-tree form's live
    /// edit/undo state.
    forms: RefCell<HashMap<WidgetId, FormController>>,
}

impl LayoutContent {
    fn new(node: LayoutNode) -> Self {
        Self {
            node,
            hits: RefCell::new(Vec::new()),
            forms: RefCell::new(HashMap::new()),
        }
    }

    /// Paints the subtree into `area`, recording the result into
    /// [`Self::hits`] and syncing any nested `"form"` extra leaf into
    /// [`Self::forms`]. Mirrors [`InteractionEngine::sync_forms`]'s own
    /// body, with no `form_rects`/`form_order` of its own: a `"form"`
    /// leaf's rect is already in the hits (each `PaintedHit::Extra`'s own
    /// `area`), and iterating the hits themselves (rather than a
    /// separately-tracked order) gives the same deterministic "paint
    /// order" guarantee that field exists to provide.
    ///
    /// `extra_stops` are focusable rects belonging to the overlay's own
    /// chrome rather than the subtree — a layout dialog's buttons — so
    /// keyboard focus can be seeded onto one of those when the subtree
    /// itself has no focusable leaf at all.
    fn render(&self, backend: &mut dyn Backend, area: Rect, extra_stops: &[(WidgetId, Rect)]) {
        let mut painted = Vec::new();
        paint_layout(backend, area, &self.node, &mut painted, &|_| {
            InteractionState::default()
        });
        let mut seen = Vec::new();
        for hit in &painted {
            let PaintedHit::Extra {
                id, area, extra, ..
            } = hit
            else {
                continue;
            };
            let Some(form) = extra_as::<Form>(extra) else {
                continue;
            };
            seen.push(*id);
            let engine_focused = backend.focus_manager().focused() == Some(id);
            let mut controllers = self.forms.borrow_mut();
            let is_new = !controllers.contains_key(id);
            let controller = controllers
                .entry(*id)
                .or_insert_with(|| FormController::new(id.as_str().to_string()));
            controller.set_form(form.clone());
            if is_new {
                controller.set_scroll_offset(form.scroll_offset);
            }
            controller.set_has_focus(form.has_focus || engine_focused);
            controller.render_and_cache(backend, *area);
        }
        self.forms.borrow_mut().retain(|id, _| seen.contains(id));

        // Keep keyboard focus inside the overlay: a just-opened one has
        // nothing clicked in it yet, and `backend`'s own focus manager
        // still names whatever the main tree last focused (or nothing at
        // all) — the same immediate-focus convention `FloatContent::Form`'s
        // own `set_has_focus(true)` establishes for a bare `Form` popover.
        // Checked every frame (not just the first) rather than tracked
        // with its own "already focused once" flag, since the only two
        // ways focus can legitimately change while the overlay is open —
        // `Self::dispatch`'s own click-to-focus and
        // `quadraui::runtime::preprocess_event`'s Tab-cycle, both driven
        // by the stops below — always land it on one of *this* frame's
        // own stops, so this is a no-op once that's already true.
        let stops = tab_stops_from_hits(&painted);
        let focused_inside = backend
            .focus_manager()
            .focused()
            .is_some_and(|focused| stops.iter().chain(extra_stops).any(|(id, _)| id == focused));
        if !focused_inside {
            if let Some((id, _)) = stops.iter().chain(extra_stops).next() {
                backend.set_focus(Some(*id));
            }
        }

        *self.hits.borrow_mut() = painted;
    }

    /// The subtree's own Tab stops, in paint order — the overlay-scoped
    /// counterpart of [`InteractionEngine::tab_stops`]'s main-tree
    /// derivation.
    fn tab_stops(&self) -> Vec<(WidgetId, Rect)> {
        tab_stops_from_hits(&self.hits.borrow())
    }

    /// The absolute rect this frame painted the `"form"` extra leaf with
    /// this `id` into, or `None` if `id` isn't one (or isn't present at
    /// all this frame).
    fn form_leaf_area(&self, id: WidgetId) -> Option<Rect> {
        self.hits.borrow().iter().find_map(|hit| match hit {
            PaintedHit::Extra {
                id: hid,
                area,
                extra,
                ..
            } if *hid == id && extra_as::<Form>(extra).is_some() => Some(*area),
            _ => None,
        })
    }

    /// Routes `event` into the subtree, exactly as the main tree's own
    /// `event -> hit -> semantic` dispatch would: a positioned
    /// `MouseDown`/`DoubleClick` click-focuses the leaf it landed on
    /// (mirroring [`InteractionEngine::handle`]'s own click-to-focus
    /// convention) and then resolves to that leaf's semantic event —
    /// [`crate::semantic::button_clicked_event`] for a `Button`,
    /// [`crate::semantic::semantic_event_for_hit`] for a `Tree`/`List`/
    /// `DataTable`/`TabBar`/`Switch`/... leaf, the owning
    /// [`FormController`] for a `"form"` extra leaf; a
    /// `KeyPressed`/`CharTyped`/`ClipboardPaste` routes to whichever leaf
    /// holds keyboard focus, through that same `FormController` for a
    /// `"form"` leaf and through
    /// [`crate::semantic::focus_scoped_key_event`]/
    /// [`crate::text_input::text_input_insert_event`] for every other
    /// kind.
    ///
    /// Escape is deliberately routed only into a focused `"form"` leaf
    /// (never wrapped into a focused `Tree`/`List`/`TabBar`'s own
    /// `KeyPressed` variant the way any other key is): the overlay itself
    /// owns Escape — dismissing a popover, resolving a dialog to its
    /// cancel button — and only an open `ComboBox` field inside the
    /// subtree outranks that.
    ///
    /// `None` means nothing in the subtree claimed `event`, leaving the
    /// caller's own fallback (Escape dismissal, modal swallowing) to run.
    fn dispatch(
        &self,
        event: &UiEvent,
        backend: &mut dyn Backend,
        sink: &mut dyn EventSink,
        text_editors: &TextEditorCache,
    ) -> Option<quadraui::Reaction> {
        use quadraui::Reaction;

        match event {
            UiEvent::MouseDown { position, .. } | UiEvent::DoubleClick { position, .. } => {
                let position = *position;
                let double_click = matches!(event, UiEvent::DoubleClick { .. });
                let modifiers = match event {
                    UiEvent::MouseDown { modifiers, .. } => *modifiers,
                    _ => Modifiers::default(),
                };
                let (id, detail, is_focusable) = {
                    let hits = self.hits.borrow();
                    let (id, detail) = hit_test_painted(&hits, position)?;
                    let is_focusable = tab_stops_from_hits(&hits).iter().any(|(t, _)| *t == id);
                    (id, detail, is_focusable)
                };

                let mut reaction = Reaction::Continue;
                if is_focusable && backend.set_focus(Some(id)) {
                    reaction = sink.dispatch(&UiEvent::FocusChanged(Some(id)), None);
                    if matches!(reaction, Reaction::Exit) {
                        return Some(reaction);
                    }
                }
                if matches!(detail, HitDetail::ButtonBody) {
                    return Some(reaction.merge(Reaction::Redraw).merge(
                        sink.dispatch(&button_clicked_event(id), Some(&HitDetail::ButtonBody)),
                    ));
                }
                if let Some(area) = self.form_leaf_area(id) {
                    let routed = self.route_form_event(id, area, event, backend, sink);
                    return Some(match routed {
                        Some(r) => reaction.merge(r),
                        None => reaction.merge(Reaction::Redraw),
                    });
                }
                let semantic = semantic_event_for_hit(
                    id,
                    &detail,
                    modifiers,
                    double_click,
                    &self.hits.borrow(),
                    position,
                );
                let resolved = semantic.unwrap_or_else(|| event.clone());
                Some(
                    reaction
                        .merge(Reaction::Redraw)
                        .merge(sink.dispatch(&resolved, Some(&detail))),
                )
            }
            UiEvent::KeyPressed { key, modifiers, .. } => {
                let focused = backend.focus_manager().focused().copied()?;
                if let Some(area) = self.form_leaf_area(focused) {
                    return self.route_form_event(focused, area, event, backend, sink);
                }
                if matches!(key, Key::Named(NamedKey::Escape)) {
                    return None;
                }
                let resolved = focus_scoped_key_event(
                    focused,
                    key,
                    *modifiers,
                    &self.hits.borrow(),
                    text_editors,
                );
                // `focus_scoped_key_event` echoes the original
                // `KeyPressed` back when `focused` names no leaf in this
                // subtree at all (or one with no key semantics of its
                // own) — nothing in here claimed it, so report that
                // rather than dispatching a duplicate of the event the
                // caller is about to fall through with.
                if resolved == *event {
                    return None;
                }
                Some(Reaction::Redraw.merge(sink.dispatch(&resolved, None)))
            }
            UiEvent::CharTyped(ch) => {
                let focused = backend.focus_manager().focused().copied()?;
                if let Some(area) = self.form_leaf_area(focused) {
                    return self.route_form_event(focused, area, event, backend, sink);
                }
                let resolved = text_input_insert_event(
                    focused,
                    &self.hits.borrow(),
                    EditOp::InsertChar(*ch),
                    text_editors,
                )?;
                Some(Reaction::Redraw.merge(sink.dispatch(&resolved, None)))
            }
            UiEvent::ClipboardPaste(text) => {
                let focused = backend.focus_manager().focused().copied()?;
                if let Some(area) = self.form_leaf_area(focused) {
                    return self.route_form_event(focused, area, event, backend, sink);
                }
                let resolved = text_input_insert_event(
                    focused,
                    &self.hits.borrow(),
                    EditOp::InsertText(text.clone()),
                    text_editors,
                )?;
                Some(Reaction::Redraw.merge(sink.dispatch(&resolved, None)))
            }
            _ => None,
        }
    }

    /// Routes `event` into the overlay-scoped [`FormController`] cached
    /// for `id` (a `"form"` extra leaf [`Self::dispatch`] already resolved
    /// `id`/`area` for), translating its [`Handled<FormControllerAction>`]
    /// into the same `UiEvent::Form`/[`quadraui::Reaction`] shape a
    /// main-tree `"form"` leaf already produces. `None` for
    /// `Handled::Ignored` (or no such controller at all) — the caller's
    /// own fallback keeps running.
    fn route_form_event(
        &self,
        id: WidgetId,
        area: Rect,
        event: &UiEvent,
        backend: &mut dyn Backend,
        sink: &mut dyn EventSink,
    ) -> Option<quadraui::Reaction> {
        use quadraui::Reaction;

        let outcome = self
            .forms
            .borrow_mut()
            .get_mut(&id)
            .map(|controller| controller.handle_event(event, backend, area))?;
        match outcome {
            Handled::Event(FormControllerAction::FormAction(form_event)) => {
                Some(Reaction::Redraw.merge(sink.dispatch(&UiEvent::Form(id, form_event), None)))
            }
            Handled::Event(FormControllerAction::ScrollChanged) | Handled::Consumed => {
                Some(Reaction::Redraw)
            }
            Handled::Ignored => None,
        }
    }
}

/// [`InteractionEngine::tab_stops`]'s own derivation, factored out so it
/// can run against either [`InteractionEngine::hit_map`] (the main tree)
/// or a `Layout`-content popover's own
/// [`FloatContent::Layout`]-scoped hits — same filter, same order, just a
/// different `hits` slice. `Split`/`SplitTree`/`Panel`/`SidebarPanel` are
/// chrome around other content, not focusable leaves themselves, so they
/// are skipped (their own action/toolbar buttons aren't recorded as
/// separate `PaintedHit` entries — see `crate::hit::HitDetail::PanelAction`
/// et al. — so a future keyboard-nav pass over those specific buttons
/// still has nothing to pick up here); `Extra` is a tab stop only when its
/// descriptor opts in via [`crate::extra_paint::ExtraPaint::tab_stop`]
/// (currently `Switch` and `TextInput`); `TextDisplay` is always one
/// (scroll/copy need keyboard focus the same way a `Tree`/`List` does);
/// every other variant yields its id and bounding `Rect`.
fn tab_stops_from_hits(hits: &[PaintedHit]) -> Vec<(WidgetId, Rect)> {
    hits.iter()
        .filter_map(|hit| match hit {
            PaintedHit::Button { id, layout } => Some((*id, layout.bounds)),
            PaintedHit::Tree { id, area, .. }
            | PaintedHit::List { id, area, .. }
            | PaintedHit::DataTable { id, area, .. }
            | PaintedHit::TabBar { id, area, .. }
            | PaintedHit::TextDisplay { id, area, .. } => Some((*id, *area)),
            PaintedHit::Split { .. }
            | PaintedHit::SplitTree { .. }
            | PaintedHit::Panel { .. }
            | PaintedHit::SidebarPanel { .. } => None,
            // A `Scroll` is chrome around its own content, like
            // `Split`/`Panel`/`SidebarPanel` above -- its content's own
            // leaves are the tab stops, reached via their own
            // `PaintedHit` arms once scrolled into view.
            PaintedHit::Scroll { .. } => None,
            PaintedHit::Extra {
                id, area, extra, ..
            } => extra.tab_stop().then_some((*id, *area)),
        })
        .collect()
}

// ─── `Form.focused_field`, uncontrolled by default ─────────────────────────
//
// `Form.focused_field: Option<WidgetId>` already has a real "unset" state
// at the type level — unlike `Split::ratio`/`TabItem::is_active` (plain
// `f32`/`bool`, needing `host::resolved_tab_active_index`'s own
// baseline-latch workaround since neither has an "uncontrolled" sentinel
// to compare against), it's the exact same shape `LayoutScroll::offset`
// already resolves through `WidgetState::resolve` directly — see
// `host::resolve_scroll_offset`. `resolve`'s own generic signature takes
// `Option<V>` as "is this controlled"; here `V` is itself `Option<WidgetId>`
// (the stored/resolved type, since "no field focused" is a legitimate
// resting state, not a sentinel to special-case), so `form.focused_field`
// is lifted one level via `.map(Some)` before being passed in: `None`
// (uncontrolled) passes straight through, `Some(id)` (controlled) becomes
// `Some(Some(id))` — [`WidgetState::resolve`]'s own `Some(value)` arm then
// pins `value = Some(id)` outright, every frame, overwriting whatever an
// engine-driven move stored in between (same strong pin
// `host::resolve_scroll_offset`'s own controlled case gets, not the
// weaker "only pins the frame it changes" baseline-latch one).

/// Resolves `form.focused_field` for [`InteractionEngine::sync_forms`] to
/// feed its cached [`FormController`] — the `Form` counterpart of
/// `host::resolve_scroll_offset`. `Some(id)` (controlled) wins outright
/// every frame and is written back, so a later frame that goes back to
/// `None` resumes from it rather than an older engine-owned value. `None`
/// (uncontrolled) resolves to whatever [`InteractionEngine::handle`]'s
/// own Tab/Shift-Tab/Up/Down/click routing last persisted via
/// [`apply_form_focus_changed`] (or stays `None` the first time this form
/// id is seen).
fn resolve_form_focused_field(widget_state: &WidgetState, form: &Form) -> Option<WidgetId> {
    widget_state.resolve(form.id, form.focused_field.map(Some), || None)
}

/// Persists `new_field` as `form_id`'s own engine-owned focused field —
/// call whenever [`InteractionEngine::handle`]'s own form-focus routing
/// (Tab/Shift-Tab between fields, Up/Down between fields, or a field
/// click) decides on a new one. A later [`resolve_form_focused_field`]
/// call overwrites this the moment the matching `Form.focused_field` is
/// `Some` again — mirrors `host::apply_scroll_offset`'s own "the next
/// resolve decides whether this sticks" shape.
fn apply_form_focus_changed(widget_state: &WidgetState, form_id: WidgetId, new_field: WidgetId) {
    widget_state.set(form_id, Some(new_field));
}

/// Whether `form`'s current `focused_field` sits at the end `delta` would
/// move past — `delta > 0` (Tab) checks the last entry of `stops`,
/// `delta < 0` (Shift-Tab) the first — or there is nothing in `stops` to
/// move between at all. `focused_field: None` is never itself a boundary
/// in either direction: [`quadraui::compose::form_controller::FormController::move_focus`]
/// already resolves `None` to the first field for Tab and the last for
/// Shift-Tab, the same "enter at the end you came from" behavior a
/// boundary hit at either real end gets — so this only forwards the key
/// into the controller for it to do that, rather than second-guessing it
/// into "leave the form" (Shift-Tab) while Tab correctly stays. Mirrors
/// [`quadraui::compose::form_dialog::FormDialogController::navigate`]'s
/// own boundary check (which decides whether to forward a Tab/Shift-Tab
/// keypress into its embedded form or hand focus to its own button row
/// instead); this is the main-tree counterpart
/// [`InteractionEngine::handle_form_tab`] uses to decide whether to hand
/// focus to the next/previous *app-level* tab stop instead.
fn form_focus_at_boundary(form: &Form, stops: &[WidgetId], delta: isize) -> bool {
    if stops.is_empty() {
        return true;
    }
    match form
        .focused_field
        .and_then(|id| stops.iter().position(|s| *s == id))
    {
        Some(i) if delta > 0 => i + 1 >= stops.len(),
        Some(0) if delta < 0 => true,
        _ => false,
    }
}

/// What [`InteractionEngine::handle_form_tab`] decided for a Tab/
/// Shift-Tab keypress reaching a focused main-tree `"form"` extra.
enum FormTabOutcome {
    /// Engine focus moved off the form entirely, to the next/previous
    /// app-level tab stop — the `Reaction` already reflects every side
    /// effect of that move (the `FocusChanged` dispatch, any "scroll
    /// fully into view", and a forced redraw), so the caller returns it
    /// immediately rather than continuing to process the original
    /// `KeyPressed`.
    LeftForm(quadraui::Reaction),
    /// Moved between fields inside the form — the caller converts the
    /// original `KeyPressed` into `UiEvent::Form(form_id, _)` and lets
    /// the rest of [`InteractionEngine::handle`] dispatch it normally,
    /// same as every other `FormEvent` a focused form produces.
    Moved(FormEvent),
    /// Nothing changed (no cached controller for this id, or its own
    /// `handle_event` had no use for the synthesized key) — the caller
    /// leaves the original `KeyPressed` untouched.
    PassThrough,
}

/// `Reaction::Continue`, unless a tooltip auto-dismissed earlier in the
/// same [`InteractionEngine::handle`] call, in which case the redraw that
/// takes it off screen still has to happen — the same reasoning
/// `InteractionEngine::handle`'s own `tooltip_dismissed` local documents,
/// applied to an early `return` that would otherwise skip past the final
/// `Reaction` merge entirely.
fn continue_or_tooltip_redraw(tooltip_dismissed: bool) -> quadraui::Reaction {
    if tooltip_dismissed {
        quadraui::Reaction::Redraw
    } else {
        quadraui::Reaction::Continue
    }
}

/// The minimal [`ScrollViewEvent::Scrolled`] that brings `[start, start +
/// len)` fully inside `view`'s own [`ScrollView::visible_range`] — scrolls
/// backward just enough when the child starts before it, forward just
/// enough when it ends after, through [`ScrollView::scroll_by`] rather
/// than computing a raw offset by hand (so the result is still clamped by
/// that primitive's own math). `None` when the child is already fully
/// visible. When `len` alone is larger than `view`'s own
/// [`ScrollView::visible_len`], the child can never be made fully visible
/// either way — `start < visible_start` still scrolls backward to align
/// its start, same as any other too-tall child. Called whenever keyboard
/// focus moves to a child of a `Scroll` — a click on one painted only
/// partly inside its enclosing viewport (straddling an edge), or a
/// directly-dispatched `UiEvent::FocusChanged` (e.g. a binding's own
/// Tab-cycling) landing on one scrolled fully out of view, since
/// [`PaintedHit::Scroll::children`] covers every child regardless of
/// whether it's currently painted.
fn scroll_child_into_view(
    view: &quadraui::ScrollView,
    start: f32,
    len: f32,
) -> Option<ScrollViewEvent> {
    let visible_start = view.scroll_offset;
    let visible_end = visible_start + view.visible_len();
    if start < visible_start {
        view.scroll_by(start - visible_start)
    } else if start + len > visible_end {
        view.scroll_by(start + len - visible_end)
    } else {
        None
    }
}

/// Whether the leaf hit for `focused` already claims PageUp/PageDown/Home/
/// End for its own semantics — a focused `TextInput`'s caret (`Home`/`End`
/// via [`crate::text_input::text_input_key_event`]) or a focused
/// `TextDisplay`'s own paging (routed through
/// [`crate::semantic::focus_scoped_key_event`] into
/// `UiEvent::TextDisplay(.., KeyPressed)`). Checked before
/// [`InteractionEngine::handle`] lets an enclosing `Scroll` claim those
/// keys instead, so a scrollable form (the canonical use for a `Scroll`
/// wrapping a `TextInput`) never loses caret movement, and a focused
/// `TextDisplay` never loses its own page-by-page scrolling, to the
/// enclosing container.
fn focused_leaf_claims_page_keys(hit_map: &[PaintedHit], focused: WidgetId) -> bool {
    hit_map.iter().any(|hit| match hit {
        PaintedHit::TextDisplay { id, .. } => *id == focused,
        PaintedHit::Extra { id, extra, .. } => {
            *id == focused && extra_as::<TextInput>(extra).is_some()
        }
        _ => false,
    })
}

/// The one binding-specific step [`InteractionEngine::handle`] cannot do
/// itself: handing a resolved [`UiEvent`] (and, for a pointer event that
/// landed on a specific sub-area, its [`HitDetail`]) to the binding's own
/// user-code callback, and reporting back the [`quadraui::Reaction`] that
/// callback produced. A binding implements this once, around its own
/// handler-invocation machinery (a Python callable, a JS function value,
/// ...) — everything upstream of this call is identical for every binding
/// and lives in [`InteractionEngine::handle`] instead.
pub trait EventSink {
    fn dispatch(&mut self, event: &UiEvent, hit_detail: Option<&HitDetail>) -> quadraui::Reaction;
}

/// One entry in [`InteractionEngine::overlays`] — every anchored popup/
/// dialog/toast/tooltip/float/completions/rich-text-popup surface a
/// binding's own `show_*`/`dismiss_*` methods can open, now backed by one
/// `Vec` instead of seven separate fields. [`Overlay::rank`] fixes each
/// kind's z-index, so [`InteractionEngine::push_overlay`] always inserts a
/// new entry at the same relative position regardless of call order —
/// the exact z-order `host::render_overlays`'s old hard-coded call
/// sequence used to encode by writing its seven `render_*` calls in a
/// fixed order: context menu lowest, then float, rich-text popup,
/// completions, dialog, tooltip, toast topmost of all.
enum Overlay {
    ContextMenu(ContextMenuController),
    /// The third slot is `Some` for a content-bearing float — a
    /// "popover" opened via [`InteractionEngine::show_popover`]/
    /// [`InteractionEngine::show_popover_layout`] — whose [`FloatContent`]
    /// this engine lays out, paints, and routes events into itself, and
    /// `None` for a plain chrome-only float opened via
    /// [`InteractionEngine::show_float`], whose content (if any) is
    /// painted by the host's own render path instead — see that method's
    /// own doc.
    Float(Float, FloatMeasure, Option<FloatContent>),
    RichTextPopup(RichTextPopup, Point),
    Completions(Completions, Point),
    Dialog(MessageDialogController),
    /// A modal [`quadraui::Dialog`] hosting a [`quadraui::Form`]'s fields
    /// — opened via [`InteractionEngine::open_form_dialog`]. Shares
    /// [`Overlay::rank`] with [`Overlay::Dialog`] so only one modal
    /// dialog (of either kind) is ever open at once. The `WidgetId` is
    /// [`FormDialogController::dialog`]'s own `id`, cached at push time —
    /// that method rebuilds a whole [`quadraui::Dialog`] (cloning title/
    /// body/buttons) to expose a field that never changes after
    /// construction, so open/close read it from here instead of paying
    /// that cost on every call.
    FormDialog(WidgetId, FormDialogController),
    /// A modal [`Dialog`] hosting an arbitrary [`LayoutNode`] subtree in
    /// its body — opened via [`InteractionEngine::open_layout_dialog`].
    /// Shares [`Overlay::rank`] with [`Overlay::Dialog`] and
    /// [`Overlay::FormDialog`], so only one modal dialog of any kind is
    /// ever open at once.
    LayoutDialog(LayoutDialog),
    Tooltip(Tooltip, Rect),
    /// Unlike every other variant, more than one of these can be in the
    /// stack at once — a toast stack is itself a group, not a singleton
    /// overlay; [`InteractionEngine::push_overlay`] only ever replaces an
    /// existing entry with the same [`quadraui::types::WidgetId`], same
    /// as [`InteractionEngine::show_toast`]'s own doc already promised.
    Toast(Toast),
}

impl Overlay {
    /// Fixed z-priority: lower sorts first (bottom of the stack, painted
    /// first). [`InteractionEngine::render_overlays`] walks
    /// [`InteractionEngine::overlays`] in this order; its overlay-input
    /// dispatch walks the same stack in reverse, so the highest-ranked
    /// open overlay always gets first claim on an event — "hit
    /// precedence agrees with paint order", the invariant every one of
    /// these kinds' own doc already promised before they shared one
    /// stack.
    fn rank(&self) -> u8 {
        match self {
            Overlay::ContextMenu(_) => 0,
            Overlay::Float(..) => 1,
            Overlay::RichTextPopup(..) => 2,
            Overlay::Completions(..) => 3,
            Overlay::Dialog(_) => 4,
            Overlay::FormDialog(..) => 4,
            Overlay::LayoutDialog(_) => 4,
            Overlay::Tooltip(..) => 5,
            Overlay::Toast(_) => 6,
        }
    }
}

/// What a content-bearing [`Overlay::Float`] embeds in the float's own
/// [`quadraui::FloatLayout::content_bounds`] — either a single
/// [`quadraui::Form`]'s fields ([`InteractionEngine::show_popover`]) or an
/// arbitrary [`LayoutNode`] subtree ([`InteractionEngine::show_popover_layout`]).
enum FloatContent {
    /// Routed through an embedded [`FormController`], the same
    /// self-contained model [`Overlay::FormDialog`] uses for its own
    /// form. See [`InteractionEngine::show_popover`]'s own doc.
    Form(FormController),
    /// An arbitrary [`LayoutNode`] subtree, painted and routed through the
    /// shared [`LayoutContent`] — the same embedding
    /// [`Overlay::LayoutDialog`] uses for its own content. See
    /// [`InteractionEngine::show_popover_layout`]'s own doc.
    Layout(LayoutContent),
}

/// A modal [`Dialog`] whose body hosts an arbitrary [`LayoutNode`] subtree
/// — opened via [`InteractionEngine::open_layout_dialog`]. The dialog's
/// own title/button chrome is painted by [`Backend::draw_dialog`] exactly
/// as for any other dialog; the subtree is painted into the slot
/// [`Self::content_bounds`] reserves inside [`quadraui::DialogLayout::body_bounds`].
struct LayoutDialog {
    /// The chrome descriptor: title, body text lines above the content
    /// slot, buttons, severity.
    dialog: Dialog,
    /// The embedded subtree.
    content: LayoutContent,
    /// The content slot's requested size — see [`DialogContentSize`].
    size: DialogContentSize,
    /// The button rects the most recent [`InteractionEngine::render_overlays`]
    /// laid out, cached so [`InteractionEngine::tab_stops`] (which has no
    /// `Backend` of its own to re-measure with) can report the dialog's
    /// own buttons as Tab stops alongside the subtree's.
    button_stops: RefCell<Vec<(WidgetId, Rect)>>,
}

/// The size a [`InteractionEngine::open_layout_dialog`] dialog's own
/// content slot asks for, in the backend's own coordinate units (cells on
/// TUI, pixels elsewhere). `None` on either axis takes that axis's
/// default: half the viewport's width clamped to 30..64 characters (the
/// same band [`FormDialogController`] sizes itself in) and a third of the
/// viewport's height. Both axes are additive — a caller that wants the
/// defaults passes [`Default::default`].
#[derive(Debug, Clone, Copy, Default)]
pub struct DialogContentSize {
    /// Full dialog-box width, chrome included.
    pub width: Option<f32>,
    /// Height of the content slot alone, excluding the dialog's own
    /// title/body-text/button chrome.
    pub height: Option<f32>,
}

/// What [`dispatch_layout_dialog`] resolved an event to.
enum LayoutDialogOutcome {
    /// The user activated the dialog button with this id (clicked it,
    /// pressed Enter/Space on it while focused, or pressed Escape with
    /// it as the cancel/default fallback). The caller closes the dialog
    /// and emits [`UiEvent::MessageDialogResolved`].
    Resolved(WidgetId),
    /// The dialog (or its content subtree) consumed the event.
    Claimed(quadraui::Reaction),
    /// Nothing about this event concerns the dialog — a window-lifecycle
    /// event, an async `User(..)` payload, a `MouseMoved` — so it must
    /// keep flowing to the app's own handler even while the modal is up.
    Ignored,
}

impl LayoutDialog {
    /// The dialog's full layout for `backend`'s current viewport, with a
    /// body slot tall enough for the body text lines *plus* the content
    /// subtree. Mirrors [`FormDialogController`]'s own `layout` — the
    /// "one layout fn, two callers" pattern, so paint and hit-test can
    /// never disagree.
    fn layout(&self, backend: &dyn Backend) -> quadraui::DialogLayout {
        let m = backend.measure();
        let lh = m.line_height;
        let char_w = m.char_width;
        let viewport = backend.viewport();
        let has_title = self.dialog.title.spans.iter().any(|s| !s.text.is_empty());
        let button_width = self
            .dialog
            .buttons
            .iter()
            .map(|b| b.resolved_button_spec().min_cell_width() * char_w)
            .fold(char_w * 10.0, f32::max);
        let width = self
            .size
            .width
            .unwrap_or_else(|| (viewport.width * 0.5).clamp(char_w * 30.0, char_w * 64.0));
        let content_height = self.size.height.unwrap_or(viewport.height / 3.0);
        let measure = quadraui::DialogMeasure {
            width,
            title_height: if has_title { lh } else { 0.0 },
            body_height: lh * self.dialog.body.len() as f32 + content_height,
            table_height: 0.0,
            input_height: 0.0,
            button_row_height: lh,
            button_width,
            button_gap: char_w * 2.0,
            padding: lh,
        };
        self.dialog.layout(viewport_rect(backend), measure, |_| {
            quadraui::ToolbarItemMeasure::new(0.0)
        })
    }

    /// The slot inside [`quadraui::DialogLayout::body_bounds`] the content
    /// subtree paints into: everything below the dialog's own body text
    /// lines, which [`Backend::draw_dialog`] paints from the top of that
    /// rect downward.
    fn content_bounds(&self, backend: &dyn Backend, layout: &quadraui::DialogLayout) -> Rect {
        let text_height = backend.measure().line_height * self.dialog.body.len() as f32;
        let body = layout.body_bounds;
        Rect::new(
            body.x,
            body.y + text_height,
            body.width,
            (body.height - text_height).max(0.0),
        )
    }

    /// Paints the dialog chrome and then the content subtree, caching
    /// this frame's button rects for [`InteractionEngine::tab_stops`].
    fn render(&self, backend: &mut dyn Backend) {
        let layout = self.layout(backend);
        backend.draw_dialog(&self.dialog, &layout);
        let stops: Vec<(WidgetId, Rect)> = layout
            .visible_buttons
            .iter()
            .map(|b| (b.id, b.bounds))
            .collect();
        let content_area = self.content_bounds(backend, &layout);
        self.content.render(backend, content_area, &stops);
        *self.button_stops.borrow_mut() = stops;
    }

    /// [`quadraui::Dialog::cancel_button_id`], falling back to
    /// [`quadraui::Dialog::default_button_id`] and then to the last
    /// button — Escape's "there's always something to resolve to"
    /// fallback, the same order [`FormDialogController`] uses.
    fn cancel_or_default_button_id(&self) -> Option<WidgetId> {
        self.dialog
            .buttons
            .iter()
            .find(|b| b.is_cancel)
            .or_else(|| self.dialog.buttons.iter().find(|b| b.is_default))
            .or_else(|| self.dialog.buttons.last())
            .map(|b| b.id)
    }
}

/// Clears `backend`'s keyboard focus when it names one of `content`'s own
/// leaves — called as a content-bearing overlay closes, so the id of a
/// leaf that no longer paints anywhere can't outlive it as "the focused
/// widget" (leaving the main tree with a focused id none of its own tab
/// stops match, and so no focus ring and no focus-scoped key routing at
/// all). Leaves focus alone when it already points at something outside
/// the overlay.
fn release_content_focus(backend: &mut dyn Backend, content: &LayoutContent) {
    let focused = backend.focus_manager().focused().copied();
    let inside = focused.is_some_and(|id| content.tab_stops().iter().any(|(stop, _)| *stop == id));
    if inside {
        backend.set_focus(None);
    }
}

/// [`InteractionEngine::dispatch_overlays`]'s own routing for an open
/// [`Overlay::LayoutDialog`], as a free function so it can run while the
/// overlay stack is immutably borrowed (closing the dialog needs
/// `&mut self`, so the caller acts on [`LayoutDialogOutcome::Resolved`]
/// only after that borrow ends).
///
/// Precedence: a click on a dialog button resolves it; Enter/Space on a
/// keyboard-focused button does the same; every other input event routes
/// into the content subtree first ([`LayoutContent::dispatch`]), and an
/// Escape the subtree had no use for resolves the dialog to its
/// cancel/default button. Any input event none of that claims is still
/// swallowed — this is a modal, so a click outside it or a key with no
/// meaning must not reach the tree painted underneath — while every
/// non-input event ([`LayoutDialogOutcome::Ignored`]) keeps flowing to the
/// app's own handler.
fn dispatch_layout_dialog(
    dialog: &LayoutDialog,
    event: &UiEvent,
    backend: &mut dyn Backend,
    sink: &mut dyn EventSink,
    text_editors: &TextEditorCache,
) -> LayoutDialogOutcome {
    use quadraui::Reaction;

    match event {
        UiEvent::MouseDown { position, .. }
        | UiEvent::MouseUp { position, .. }
        | UiEvent::DoubleClick { position, .. }
        | UiEvent::Scroll { position, .. } => {
            let position = *position;
            if matches!(
                event,
                UiEvent::MouseDown { .. } | UiEvent::DoubleClick { .. }
            ) {
                let layout = dialog.layout(backend);
                if let quadraui::DialogHit::Button(id) = layout.hit_test(position.x, position.y) {
                    return LayoutDialogOutcome::Resolved(id);
                }
            }
            match dialog.content.dispatch(event, backend, sink, text_editors) {
                Some(reaction) => LayoutDialogOutcome::Claimed(reaction),
                None => LayoutDialogOutcome::Claimed(Reaction::Continue),
            }
        }
        UiEvent::KeyPressed { key, .. } => {
            let activates = matches!(key, Key::Named(NamedKey::Enter) | Key::Char(' '));
            if activates {
                let focused = backend.focus_manager().focused().copied();
                let on_button = focused.filter(|id| {
                    dialog
                        .button_stops
                        .borrow()
                        .iter()
                        .any(|(button, _)| button == id)
                });
                if let Some(id) = on_button {
                    return LayoutDialogOutcome::Resolved(id);
                }
            }
            if let Some(reaction) = dialog.content.dispatch(event, backend, sink, text_editors) {
                return LayoutDialogOutcome::Claimed(reaction);
            }
            if matches!(key, Key::Named(NamedKey::Escape)) {
                if let Some(id) = dialog.cancel_or_default_button_id() {
                    return LayoutDialogOutcome::Resolved(id);
                }
            }
            LayoutDialogOutcome::Claimed(Reaction::Continue)
        }
        UiEvent::CharTyped(..) | UiEvent::ClipboardPaste(..) => {
            match dialog.content.dispatch(event, backend, sink, text_editors) {
                Some(reaction) => LayoutDialogOutcome::Claimed(reaction),
                None => LayoutDialogOutcome::Claimed(Reaction::Continue),
            }
        }
        _ => LayoutDialogOutcome::Ignored,
    }
}

/// Owns every piece of state the `event -> hit -> semantic` orchestration
/// needs across frames: the current frame's painted hits, an armed
/// divider/scrollbar drag, per-widget text-input cursor cache, and the
/// menu-bar/context-menu controllers. A binding's own `AppLogic` impl holds
/// exactly one of these and never needs its own copy of any of this state.
#[derive(Default)]
pub struct InteractionEngine {
    /// Every primitive the binding's most recent [`quadraui::AppLogic::render`]
    /// call painted, in paint order, as a [`PaintedHit`] carrying its id and
    /// the hit-testable layout its backend `draw_*` call returned. A binding
    /// rebuilds this from scratch via [`InteractionEngine::set_hits`] at the
    /// start of every `render` call; [`InteractionEngine::handle`] reads it
    /// to translate an incoming pointer event's screen coordinates into the
    /// `WidgetId` (and finer [`HitDetail`]) it landed on.
    hit_map: RefCell<Vec<PaintedHit>>,
    /// Armed by a `MouseDown` landing on a split divider — a two-pane
    /// `Split`'s (per [`Self::hit_map`]'s [`PaintedHit::Split`]
    /// hit-testing) or one of an N-way `SplitTree`'s (per
    /// [`PaintedHit::SplitTree`]) — and cleared on the matching `MouseUp`.
    /// While armed, [`InteractionEngine::handle`] turns every `MouseMoved`
    /// into a synthesized `UiEvent::SplitDividerDragged` instead of
    /// forwarding the raw move, so the binding's own event handler never
    /// has to recompute split geometry itself to support divider dragging.
    active_drag: RefCell<Option<ActiveDrag>>,
    /// Armed by a `MouseDown` landing on `HitDetail::TreeScrollbarThumb`
    /// (per [`Self::hit_map`]'s [`PaintedHit::Tree`] hit-testing) and cleared on
    /// the matching `MouseUp`. While armed, [`InteractionEngine::handle`]
    /// turns every `MouseMoved` into a synthesized `UiEvent::ScrollOffsetChanged`
    /// instead of forwarding the raw move, mirroring [`Self::active_drag`]'s
    /// divider-drag handling for the tree's own scrollbar thumb.
    scrollbar_drag: RefCell<Option<ScrollbarDrag>>,
    /// Armed by a `MouseDown` landing on `HitDetail::DataTableHeaderDivider`
    /// (per [`Self::hit_map`]'s [`PaintedHit::DataTable`] hit-testing) and
    /// cleared on the matching `MouseUp`. While armed,
    /// [`InteractionEngine::handle`] turns every `MouseMoved` into a
    /// synthesized `UiEvent::DataTable(.., ColumnResized)` instead of
    /// forwarding the raw move, mirroring [`Self::active_drag`]'s divider
    /// handling for a `DataTable`'s own column dividers.
    column_drag: RefCell<Option<DataTableColumnDrag>>,
    /// Armed by a `MouseDown` landing on `HitDetail::ScrollScrollbarThumb`
    /// (per [`Self::hit_map`]'s [`PaintedHit::Scroll`] hit-testing) and
    /// cleared on the matching `MouseUp`. While armed,
    /// [`InteractionEngine::handle`] turns every `MouseMoved` into a
    /// synthesized `UiEvent::ScrollOffsetChanged` instead of forwarding the
    /// raw move, mirroring [`Self::scrollbar_drag`]'s own thumb-drag
    /// handling for a `LayoutNode::Scroll` container's scrollbar instead of
    /// a `Tree`'s — kept as its own field rather than reused because a
    /// `Scroll` can scroll along either axis (see [`ScrollDrag::axis`]'s
    /// own doc), unlike a `Tree`'s always-vertical one.
    scroll_drag: RefCell<Option<ScrollDrag>>,
    /// Per-`TextInput` [`quadraui::TextEditor`] session, keyed by
    /// `WidgetId`, that survives across keystrokes — see
    /// [`crate::text_input::apply_text_input_edit`]'s own doc for why a
    /// cache is needed at all, including its own `undo_stack`.
    text_editors: TextEditorCache,
    /// Framework-owned per-widget state store, keyed by `WidgetId`,
    /// surviving across frames — see [`crate::widget_state::WidgetState`]'s
    /// own module doc for its controlled/uncontrolled resolution and GC
    /// policies. [`Self::set_hits`] sweeps it every frame alongside
    /// [`Self::text_editors`]'s own pruning. [`Self::handle`]'s own
    /// `KeyPressed` routing writes a focused `Tree`'s cursor through it via
    /// [`crate::host::apply_tree_nav_key`]; a binding's own `render` reads
    /// it back (merged with whatever the descriptor pins) via
    /// [`crate::host::resolve_tree_cursor`]. [`Self::handle`]'s final
    /// dispatch also writes a divider drag's ratio, a clicked tab's active
    /// index, and a toggled tree row's expand state through
    /// [`crate::host::apply_split_divider_drag`]/[`crate::host::apply_tab_activated`]/
    /// [`crate::host::apply_tree_toggle_expand`]; a binding's own `render`
    /// reads each back the same way, via
    /// [`crate::host::resolve_split_ratio`]/[`crate::host::resolve_tab_bar_active`]/
    /// [`crate::host::resolve_tree_expansion`].
    widget_state: WidgetState,
    /// Owns the in-window dropdown behind a `"menu_bar"` extra: the
    /// open/close/keyboard-nav/submenu state machine that turns a click on
    /// a top-level item into an open dropdown and a selection into
    /// `UiEvent::MenuActivated`.
    menu_system: RefCell<MenuSystem>,
    /// The absolute bounds of the `"menu_bar"` extra [`InteractionEngine::sync_menu_bar`]
    /// most recently painted, read by [`InteractionEngine::handle`] to call
    /// [`MenuSystem::handle`] with the same `bar_rect` the paint pass used.
    /// `None` whenever no `"menu_bar"` extra was present that frame.
    menu_bar_rect: RefCell<Option<Rect>>,
    /// The one z-ordered overlay stack backing every `show_*`/`dismiss_*`
    /// surface below (context menu, dialog, toast, tooltip, completions,
    /// float, rich-text popup) — see [`Overlay`]'s own doc. Lowest
    /// [`Overlay::rank`] first; [`Self::push_overlay`]/[`Self::remove_overlay`]
    /// are the only places that mutate this directly, so every kind's
    /// own insertion/removal nuance (singleton-replace vs. toast's
    /// replace-by-id) lives in those two methods instead of being
    /// hand-rolled seven times.
    overlays: Vec<Overlay>,
    /// Per-bar hovered child, keyed by the chrome-bar extra's own id
    /// (`Toolbar`'s button, `StatusBar`'s segment, `ActivityBar`'s item) —
    /// updated on every `MouseMoved` via [`chrome_hover_hit`] and read back
    /// by [`Self::chrome_interaction`] so `render` paints real hover state
    /// instead of `InteractionState::default()`. A bar absent from this
    /// map has nothing hovered.
    chrome_hover: RefCell<HashMap<WidgetId, WidgetId>>,
    /// Per-bar pressed child, mirroring [`Self::chrome_hover`]. Set on a
    /// `MouseDown` landing on a chrome-bar child and cleared unconditionally
    /// on every `MouseUp`, same as [`Self::active_drag`]/[`Self::scrollbar_drag`]/
    /// [`Self::column_drag`].
    chrome_pressed: RefCell<HashMap<WidgetId, WidgetId>>,
    /// The whole-widget-level hover target — any painted primitive, not
    /// just a chrome-bar child (see [`Self::chrome_hover`] for that
    /// narrower case) — the most recent `MouseMoved` landed on, paired
    /// with its [`HitDetail`]. [`InteractionEngine::handle`] compares this
    /// against each new `MouseMoved`'s own `hit_test_painted` result and,
    /// when it changes (by id or by detail alone — e.g. moving from one
    /// tree row to another within the same `Tree`), dispatches a
    /// `UiEvent::MouseLeft` for the old target and a `UiEvent::MouseEntered`
    /// for the new one through `sink`, each carrying its own `HitDetail`
    /// the same way a click would — the hit-tested hover signal a
    /// binding's own toolbar/tree/details-pane hover tooltip needs,
    /// without re-deriving hit-testing from the raw position itself.
    /// `None` once the pointer has left every hoverable primitive.
    hover: RefCell<Option<(WidgetId, HitDetail)>>,
    /// One [`FormController`] per `"form"` extra painted this (or a prior)
    /// frame, keyed by the extra's own id — rebuilt from [`Self::sync_forms`]
    /// every render so a submit/button click, toggle, or keystroke inside
    /// it resolves through the SAME field-level routing a `Form`-embedding
    /// host would get from driving `FormController` directly, instead of
    /// staying paint-only. An entry survives across frames (unlike
    /// [`Self::menu_system`]'s single instance) since, unlike `"menu_bar"`/
    /// `"find_replace"`, nothing about `Form` makes it a singleton overlay —
    /// a frame can legitimately paint more than one, each under its own id.
    form_controllers: RefCell<HashMap<WidgetId, FormController>>,
    /// The absolute bounds [`Self::sync_forms`] most recently painted each
    /// [`Self::form_controllers`] entry into, read by
    /// [`InteractionEngine::handle`] so a routed event's position
    /// resolves against the same `rect` the paint pass used (`FormController::handle_event`
    /// does its own position-relative hit-testing internally, unlike this
    /// engine's `hit_map`-based primitives).
    form_rects: RefCell<HashMap<WidgetId, Rect>>,
    /// This frame's `"form"` extra ids, in the paint order
    /// [`Self::sync_forms`] saw them — the `HashMap`s above have none, so
    /// a `MouseDown` claim loop iterating `form_rects` directly would
    /// resolve a same-frame rect overlap (which should never happen in
    /// practice; [`Self::sync_forms`]'s own doc notes it) against a
    /// nondeterministic order across runs. [`InteractionEngine::handle`]
    /// walks this instead, so the invariant "at most one claims a given
    /// click" is reproducible rather than merely usually true.
    form_order: RefCell<Vec<WidgetId>>,
    /// Which of [`Self::form_controllers`]' ids [`Self::sync_forms`] most
    /// recently observed as `backend.focus_manager().focused()` itself —
    /// unlike each controller's own [`FormController::has_focus`] (which
    /// ORs in the descriptor's purely cosmetic `Form.has_focus`, see
    /// [`Self::sync_forms`]'s own doc), this tracks *only* real engine
    /// focus, so [`Self::tab_stops`] can tell "Tab should move within
    /// this form's own fields" apart from "this form merely paints
    /// itself as highlighted" without a `&dyn Backend` of its own to ask
    /// (`AppLogic::tab_stops` takes none).
    form_engine_focus: RefCell<HashSet<WidgetId>>,
    /// Which corner [`Self::overlays`]' [`Overlay::Toast`] entries stack
    /// in — set by the most recent [`Self::show_toast`] call. Not part of
    /// [`Overlay::Toast`] itself since it applies to the whole group, not
    /// a single toast — showing a second toast in a different corner
    /// moves the whole stack, same caveat [`quadraui::ToastOverlay`]'s
    /// own single-`corner` field already carried.
    toast_corner: ToastCorner,
}

impl InteractionEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// The current frame's painted hits, for a binding that needs direct
    /// read access (e.g. to resolve a tree's scrollbar state for its own
    /// tests or diagnostics) beyond what [`InteractionEngine::handle`] and
    /// [`InteractionEngine::tab_stops`] already expose.
    pub fn hit_map(&self) -> Ref<'_, Vec<PaintedHit>> {
        self.hit_map.borrow()
    }

    /// This engine's own per-widget state store — a binding's `render`
    /// passes this to [`crate::host::resolve_tree_cursor`] (right before
    /// painting a `Tree`) so the cursor [`Self::handle`]'s `KeyPressed`
    /// routing moved via [`crate::host::apply_tree_nav_key`] is what
    /// actually gets painted.
    pub fn widget_state(&self) -> &WidgetState {
        &self.widget_state
    }

    /// Replaces this frame's painted hits. Call once per render, after the
    /// binding's own paint pass (`draw_bands`/`paint_layout`) has fully
    /// populated `hits`.
    ///
    /// Also prunes [`Self::text_editors`] down to just the ids still
    /// painted as a `"text_input"` extra this frame — mirroring
    /// [`Self::sync_forms`]'s own "painted this frame" retain for
    /// [`Self::form_controllers`]. Without this, a `TextInput` that stops
    /// rendering (its host removes it, navigates away, ...) leaves its
    /// cached [`quadraui::TextEditor`] — buffer, cursor, and up to
    /// `UNDO_HISTORY_LIMIT` full undo snapshots — in the map forever.
    ///
    /// Also sweeps [`Self::widget_state`] via [`WidgetState::gc`] — its own
    /// "seen this frame" marks, not `hits`, drive what survives; see that
    /// type's own module doc.
    pub fn set_hits(&self, hits: Vec<PaintedHit>) {
        let live_text_inputs: Vec<WidgetId> = hits
            .iter()
            .filter_map(|hit| match hit {
                PaintedHit::Extra { id, extra, .. } if extra_as::<TextInput>(extra).is_some() => {
                    Some(*id)
                }
                _ => None,
            })
            .collect();
        self.text_editors
            .borrow_mut()
            .retain(|id, _| live_text_inputs.contains(id));
        self.widget_state.gc();
        *self.hit_map.borrow_mut() = hits;
    }

    /// Inserts `overlay` into [`Self::overlays`] at the position
    /// [`Overlay::rank`] fixes for its kind. Every kind but
    /// [`Overlay::Toast`] is a singleton: inserting one first removes any
    /// existing entry of the SAME kind, so a binding's own `show_*` call
    /// keeps the "replaces whatever was already open" contract each of
    /// those methods' own doc promises. A [`Overlay::Toast`] instead
    /// removes only a prior entry with the same [`quadraui::types::WidgetId`]
    /// (same "replace by id, else stack" rule [`Self::show_toast`]'s own
    /// doc already promised) and is otherwise appended after any already
    /// showing, preserving their relative (oldest-first) order.
    fn push_overlay(&mut self, overlay: Overlay) {
        match &overlay {
            Overlay::Toast(toast) => {
                let id = toast.id;
                self.overlays
                    .retain(|o| !matches!(o, Overlay::Toast(t) if t.id == id));
            }
            _ => {
                let rank = overlay.rank();
                self.overlays.retain(|o| o.rank() != rank);
            }
        }
        let rank = overlay.rank();
        let at = self
            .overlays
            .iter()
            .take_while(|o| o.rank() <= rank)
            .count();
        self.overlays.insert(at, overlay);
    }

    /// Removes the first entry matching `pred` (there is at most one,
    /// except for [`Overlay::Toast`], where a caller's own predicate
    /// narrows to a single id), returning it. Backs every `dismiss_*`/
    /// `close_*` method below — each supplies its own kind/id match, so
    /// the removal itself lives in one place instead of seven near-copies
    /// of `Option::take`/`Vec::retain`.
    fn remove_overlay(&mut self, pred: impl Fn(&Overlay) -> bool) -> Option<Overlay> {
        let idx = self.overlays.iter().position(pred)?;
        Some(self.overlays.remove(idx))
    }

    /// Whether a `Custom`-style context-menu popup opened via
    /// [`Self::open_context_menu`] is currently open — `false` for a
    /// `Native`-style one (AppKit owns that popup's own state) and when
    /// nothing is open, mirroring [`ContextMenuController::is_open`].
    fn context_menu_open(&self) -> bool {
        self.overlays
            .iter()
            .any(|o| matches!(o, Overlay::ContextMenu(c) if c.is_open()))
    }

    /// Closes the open context-menu popup (if any), via its own
    /// [`ContextMenuController::close`].
    fn close_context_menu(&mut self, backend: &mut dyn Backend) {
        if let Some(Overlay::ContextMenu(controller)) = self
            .overlays
            .iter_mut()
            .find(|o| matches!(o, Overlay::ContextMenu(_)))
        {
            controller.close(backend);
        }
    }

    /// Closes the open context-menu popup without dispatching any event.
    /// Returns whether one was actually open to close.
    pub fn dismiss_context_menu(&mut self, backend: &mut dyn Backend) -> bool {
        let was_open = self.context_menu_open();
        if was_open {
            self.close_context_menu(backend);
        }
        was_open
    }

    /// Builds an anchored [`ContextMenu`] from `items` and opens it at `at`,
    /// seeding its keyboard selection on the first selectable item — the
    /// full "build the popup, open it" sequence behind a binding's own
    /// `show_context_menu` surface, so that surface only has to decode its
    /// own wire format into `items` before calling this. Reuses the
    /// [`Overlay::ContextMenu`] entry already in [`Self::overlays`] (if
    /// any) rather than always pushing a fresh one, so calling this twice
    /// in a row still goes through the SAME [`ContextMenuController::open`]
    /// call a persisted controller would have, including whatever
    /// close-the-old-one-first behaviour that method's own `Native` path
    /// performs internally.
    pub fn open_context_menu(
        &mut self,
        items: Vec<ContextMenuItem>,
        at: Point,
        backend: &mut dyn Backend,
    ) {
        let mut menu = ContextMenu {
            id: WidgetId::new("qui-context-menu"),
            items,
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
        };
        menu.selected_idx = menu.first_selectable();
        if let Some(Overlay::ContextMenu(controller)) = self
            .overlays
            .iter_mut()
            .find(|o| matches!(o, Overlay::ContextMenu(_)))
        {
            controller.open(menu, at, backend);
            return;
        }
        let mut controller = ContextMenuController::new();
        controller.open(menu, at, backend);
        self.push_overlay(Overlay::ContextMenu(controller));
    }

    /// The context-menu popup's own event routing, forwarded to whatever
    /// [`Overlay::ContextMenu`] entry is in [`Self::overlays`] — `Ignored`
    /// immediately when none is, same contract
    /// [`ContextMenuController::handle`] already promises.
    fn dispatch_context_menu(
        &mut self,
        event: &UiEvent,
        backend: &mut dyn Backend,
    ) -> ContextMenuOutcome {
        match self.overlays.iter_mut().find_map(|o| match o {
            Overlay::ContextMenu(c) => Some(c),
            _ => None,
        }) {
            Some(controller) => controller.handle(event, backend),
            None => ContextMenuOutcome::Ignored,
        }
    }

    /// Whether a message dialog opened via [`Self::open_message_dialog`]
    /// is currently open.
    pub fn dialog_open(&self) -> bool {
        self.overlays
            .iter()
            .any(|o| matches!(o, Overlay::Dialog(_)))
    }

    /// Opens an in-canvas message/alert/confirm dialog built from `opts`,
    /// pushing its bounds onto the backend's [`quadraui::ModalStack`] (via
    /// [`quadraui::Backend::modal_stack_handle`]) so other modal-aware code
    /// gets the same hit-test precedence an open context menu already has.
    /// Replaces (and pops) any dialog already open. Also closes an open
    /// `"menu_bar"` dropdown or context-menu popup, if either is open: the
    /// dialog paints on top of both ([`Overlay::Dialog`]'s own
    /// [`Overlay::rank`] is higher than [`Overlay::ContextMenu`]'s), and
    /// [`Self::handle`] now routes the dialog ahead of both, so a popup
    /// left open underneath would be unreachable input it still holds
    /// precedence over. Call from a binding's own `show_message_dialog`
    /// surface.
    pub fn open_message_dialog(&mut self, opts: MessageDialogOptions, backend: &mut dyn Backend) {
        self.close_message_dialog(backend);
        self.close_form_dialog(backend);
        self.close_layout_dialog(backend);
        if self.menu_system.borrow().is_open() {
            self.menu_system.borrow_mut().close(backend);
        }
        if self.context_menu_open() {
            self.close_context_menu(backend);
        }
        let controller = MessageDialogController::new(opts);
        let id = controller.dialog().id;
        let bounds = controller.bounds(backend);
        backend.modal_stack_handle().borrow_mut().push(id, bounds);
        self.push_overlay(Overlay::Dialog(controller));
    }

    /// Closes the open message dialog (if any), popping its
    /// [`quadraui::ModalStack`] entry. Returns whether a dialog was open to
    /// close. Exposed so a binding's own `show_message_dialog` surface can
    /// offer a programmatic dismiss alongside input-driven resolution
    /// (Enter/Escape/click) — without this, a dialog with no buttons at
    /// all, or one the app wants to tear down from outside input, could
    /// never be cleared. A no-op (returns `false`) if nothing is open.
    pub fn close_message_dialog(&mut self, backend: &mut dyn Backend) -> bool {
        match self.remove_overlay(|o| matches!(o, Overlay::Dialog(_))) {
            Some(Overlay::Dialog(open)) => {
                backend
                    .modal_stack_handle()
                    .borrow_mut()
                    .pop(&open.dialog().id);
                true
            }
            _ => false,
        }
    }

    /// Whether a form-hosting dialog opened via [`Self::open_form_dialog`]
    /// is currently open.
    pub fn form_dialog_open(&self) -> bool {
        self.overlays
            .iter()
            .any(|o| matches!(o, Overlay::FormDialog(..)))
    }

    /// Opens `dialog` — a [`FormDialogController`] hosting a
    /// [`quadraui::Form`]'s fields inside a modal [`quadraui::Dialog`]'s own
    /// title/button chrome — pushing its bounds onto the backend's
    /// [`quadraui::ModalStack`], the same contract
    /// [`Self::open_message_dialog`] already follows. Replaces (and pops)
    /// any modal dialog already open, of any of the three kinds: they all
    /// share [`Overlay::rank`], so [`Self::push_overlay`]'s "same rank,
    /// same slot" rule keeps "only one modal dialog at a time" true
    /// across all of them. A binding's own `show_form_dialog` surface calls this, then
    /// pushes the app's current field values onto `dialog` every frame
    /// via [`FormDialogController::set_form`] before
    /// [`Self::render_overlays`] paints it — the same "app owns the
    /// data, controller owns the edits" contract a bare
    /// [`FormController`] consumer already follows.
    pub fn open_form_dialog(&mut self, dialog: FormDialogController, backend: &mut dyn Backend) {
        self.close_form_dialog(backend);
        self.close_message_dialog(backend);
        self.close_layout_dialog(backend);
        if self.menu_system.borrow().is_open() {
            self.menu_system.borrow_mut().close(backend);
        }
        if self.context_menu_open() {
            self.close_context_menu(backend);
        }
        let id = dialog.dialog().id;
        let bounds = dialog.bounds(backend);
        backend.modal_stack_handle().borrow_mut().push(id, bounds);
        self.push_overlay(Overlay::FormDialog(id, dialog));
    }

    /// Closes the open form-dialog (if any), popping its
    /// [`quadraui::ModalStack`] entry. Returns whether one was open.
    /// Mirrors [`Self::close_message_dialog`]'s own doc.
    pub fn close_form_dialog(&mut self, backend: &mut dyn Backend) -> bool {
        match self.remove_overlay(|o| matches!(o, Overlay::FormDialog(..))) {
            Some(Overlay::FormDialog(id, _)) => {
                backend.modal_stack_handle().borrow_mut().pop(&id);
                true
            }
            _ => false,
        }
    }

    /// Mutable access to the open form-dialog's own controller, if any —
    /// a binding's render path uses this to call
    /// [`FormDialogController::set_form`]/[`FormDialogController::set_body`]
    /// every frame before painting, the same "push fresh state, then
    /// render" contract a bare [`FormController`] consumer already
    /// follows.
    pub fn form_dialog_mut(&mut self) -> Option<&mut FormDialogController> {
        self.overlays.iter_mut().find_map(|o| match o {
            Overlay::FormDialog(_, d) => Some(d),
            _ => None,
        })
    }

    // ── Toast ────────────────────────────────────────────────────────

    /// Shows `toast` in `corner`, stacking it among whatever toasts are
    /// already showing. Replaces (keeps the position of, but refreshes
    /// the content of) any existing toast with the same
    /// [`quadraui::types::WidgetId`]. `corner` applies to the whole
    /// stack, so showing a second toast in a different corner moves
    /// every toast — including ones already showing — to the new
    /// corner; apps that want toasts in two corners at once need two
    /// separate `WidgetId`-disjoint calls per frame, same caveat
    /// [`quadraui::ToastOverlay`]'s own single-`corner` field carries.
    pub fn show_toast(&mut self, toast: Toast, corner: ToastCorner) {
        self.toast_corner = corner;
        self.push_overlay(Overlay::Toast(toast));
    }

    /// Removes the toast with this id, if showing. Returns whether one
    /// was removed. Exposed so a binding's own `dismiss_toast` surface
    /// can close a toast programmatically (e.g. after its own auto-expire
    /// timer fires) in addition to the dismiss `×`
    /// [`Self::handle`] already routes.
    pub fn dismiss_toast(&mut self, id: WidgetId) -> bool {
        self.remove_overlay(|o| matches!(o, Overlay::Toast(t) if t.id == id))
            .is_some()
    }

    fn toast_overlay(&self) -> ToastOverlay {
        ToastOverlay {
            id: WidgetId::new("qui-toast-stack"),
            corner: self.toast_corner,
            toasts: self
                .overlays
                .iter()
                .filter_map(|o| match o {
                    Overlay::Toast(t) => Some(t.clone()),
                    _ => None,
                })
                .collect(),
            focus: None,
        }
    }

    /// Recomputes the same [`quadraui::ToastStackLayout`]
    /// [`Self::render_overlays`] just painted, for [`Self::handle`]'s click
    /// routing — the "one layout fn, two callers" pattern
    /// [`MessageDialogController::layout`] already follows for `Dialog`.
    fn toast_layout(&self, backend: &dyn Backend) -> ToastStackLayout {
        backend.toast_stack_layout(viewport_rect(backend), &self.toast_overlay())
    }

    // ── Tooltip ──────────────────────────────────────────────────────

    /// The current hit-tested hover target — the same `(id, detail)` pair
    /// [`Self::handle`] dispatches as `MouseEntered`/`MouseLeft`, read
    /// back by a caller that wants to know what's hovered right now
    /// without re-deriving it from the raw hit map (e.g. a hover-tooltip
    /// driver deciding whether a tooltip should be pending for this
    /// frame). `None` once the pointer has left every hoverable
    /// primitive — see [`Self::hover`]'s own doc.
    pub fn hovered(&self) -> Option<(WidgetId, HitDetail)> {
        self.hover.borrow().clone()
    }

    /// Shows `tooltip` anchored against `anchor` (the rect of whatever
    /// widget it describes, in the backend's ABSOLUTE frame). Replaces
    /// any tooltip already showing.
    pub fn show_tooltip(&mut self, tooltip: Tooltip, anchor: Rect) {
        self.push_overlay(Overlay::Tooltip(tooltip, anchor));
    }

    /// Hides the open tooltip (if any). Returns whether one was open.
    /// [`Self::handle`] already does this automatically on the next
    /// click/key — see that method's doc — so apps only need this for an
    /// explicit "hide now" (e.g. the hovered widget itself changed).
    pub fn hide_tooltip(&mut self) -> bool {
        self.remove_overlay(|o| matches!(o, Overlay::Tooltip(..)))
            .is_some()
    }

    /// Whether a tooltip is currently open — the `bool` half of
    /// [`Self::show_tooltip`]/[`Self::hide_tooltip`]'s own state, read
    /// back by a caller that needs to resync its own idea of "is a
    /// tooltip showing" (e.g. a hover-tooltip driver whose tracker
    /// thinks one is visible) against whatever [`Self::handle`]'s own
    /// click/key auto-dismiss just did to it.
    pub fn tooltip_open(&self) -> bool {
        self.overlays
            .iter()
            .any(|o| matches!(o, Overlay::Tooltip(..)))
    }

    fn tooltip_layout(
        &self,
        backend: &dyn Backend,
        tooltip: &Tooltip,
        anchor: Rect,
    ) -> TooltipLayout {
        let m = backend.measure();
        let n_lines = tooltip.text.lines().count().max(1);
        let longest = tooltip.text.lines().map(str::len).max().unwrap_or(0);
        let measure = TooltipMeasure::new(
            m.char_width * (longest as f32 + 4.0),
            m.line_height * (n_lines as f32 + 2.0),
        );
        tooltip.layout(anchor, viewport_rect(backend), measure, m.line_height)
    }

    // ── Completions ──────────────────────────────────────────────────

    /// Shows `completions` anchored at `cursor` (the caret position in
    /// the backend's ABSOLUTE frame). Replaces any popup already open.
    pub fn show_completions(&mut self, completions: Completions, cursor: Point) {
        self.push_overlay(Overlay::Completions(completions, cursor));
    }

    /// Closes the open completions popup (if any) without reporting a
    /// selection. Returns whether one was open.
    pub fn dismiss_completions(&mut self) -> bool {
        self.remove_overlay(|o| matches!(o, Overlay::Completions(..)))
            .is_some()
    }

    /// The open completions popup (if any) and the cursor position it was
    /// anchored to, for [`Self::handle`]'s hit-test dispatch — cloned out
    /// of [`Self::overlays`] so the caller can go on to call
    /// [`Self::dismiss_completions`] (which needs `&mut self`) without a
    /// borrow of the stack still outstanding.
    fn find_completions(&self) -> Option<(Completions, Point)> {
        self.overlays.iter().find_map(|o| match o {
            Overlay::Completions(c, p) => Some((c.clone(), *p)),
            _ => None,
        })
    }

    fn completions_layout(
        &self,
        backend: &dyn Backend,
        completions: &Completions,
        cursor: Point,
    ) -> CompletionsLayout {
        let m = backend.measure();
        completions.layout(
            cursor.x,
            cursor.y,
            m.line_height,
            viewport_rect(backend),
            m.char_width * 30.0,
            m.line_height * 8.0,
            |_| CompletionItemMeasure::new(m.line_height),
        )
    }

    // ── Float ────────────────────────────────────────────────────────

    /// Opens `float` at `measure`'s size, pushing its resolved bounds
    /// onto the backend's [`quadraui::ModalStack`] via
    /// [`quadraui::ModalStack::push_float`] so other modal-aware code
    /// gets the same hit-test precedence [`Overlay::Dialog`] already has.
    /// Replaces (and pops) any float already open. **Content is not
    /// this call's concern** — same as [`quadraui::Backend::draw_float`]
    /// itself, the host paints whatever belongs in
    /// [`quadraui::FloatLayout::content_bounds`] through its own normal
    /// render path after this call's [`Self::render_overlays`] runs.
    pub fn show_float(&mut self, float: Float, measure: FloatMeasure, backend: &mut dyn Backend) {
        self.close_float(backend);
        let layout = float.layout(viewport_rect(backend), measure);
        backend
            .modal_stack_handle()
            .borrow_mut()
            .push_float(&float, layout.bounds);
        self.push_overlay(Overlay::Float(float, measure, None));
    }

    /// Opens `float` as a content-bearing popover: anchored and chromed
    /// exactly like [`Self::show_float`], but with `form`'s fields laid
    /// out and painted into [`quadraui::FloatLayout::content_bounds`] by
    /// this engine itself (through an embedded [`FormController`])
    /// rather than left to the host's own render path. Unlike a plain
    /// [`Self::show_float`], [`Self::handle`] routes every event into
    /// the embedded form while this is open — typing/toggling a field
    /// emits the normal `UiEvent::Form(float.id, ..)` event, same shape
    /// a `"form"` layout-tree leaf already emits — and a `MouseDown`
    /// landing outside `float`'s own bounds dismisses it, in addition
    /// to the Escape dismissal every focusable float already gets (see
    /// [`Self::dispatch_overlays`]'s own doc). `float.focusable` is
    /// forced to `true` regardless of what the caller set — a popover
    /// with no keyboard focus of its own would leave `form`'s fields
    /// permanently unreachable by keyboard. Tab/Shift+Tab stay trapped
    /// inside `form`'s own fields — see [`Self::tab_stops`]'s own doc
    /// for the other half of that trap. Replaces (and pops) any float
    /// already open. Use [`Self::popover_mut`] afterward to push fresh
    /// field values onto the still-open popover every frame, the same
    /// "app owns the data, controller owns the edits" contract a bare
    /// [`FormController`] consumer already follows — calling this again
    /// instead would recreate the embedded controller from scratch,
    /// discarding its own live cursor/selection/scroll state.
    pub fn show_popover(
        &mut self,
        mut float: Float,
        measure: FloatMeasure,
        form: Form,
        backend: &mut dyn Backend,
    ) {
        float.focusable = true;
        self.close_float(backend);
        let layout = float.layout(viewport_rect(backend), measure);
        backend
            .modal_stack_handle()
            .borrow_mut()
            .push_float(&float, layout.bounds);
        let mut controller = FormController::new(float.id.as_str().to_string());
        controller.set_form(form);
        controller.set_has_focus(true);
        self.push_overlay(Overlay::Float(
            float,
            measure,
            Some(FloatContent::Form(controller)),
        ));
    }

    /// Mutable access to a Form-content popover's embedded
    /// [`FormController`] (opened via [`Self::show_popover`]), if one is
    /// open — `None` for a plain [`Self::show_float`] float, a
    /// [`Self::show_popover_layout`] popover, or no float at all. Mirrors
    /// [`Self::form_dialog_mut`]'s own doc: a binding's render path calls
    /// [`FormController::set_form`] through this every frame before
    /// painting, the same "push fresh state, then render" contract a
    /// bare `FormController` consumer already follows — without it, a
    /// popover's fields are frozen at whatever `form` was passed to
    /// [`Self::show_popover`] when it opened, since that method's own
    /// `self.close_float(backend)` would otherwise be the only way to
    /// refresh them, discarding the controller's own live
    /// cursor/selection/scroll state in the process.
    pub fn popover_mut(&mut self) -> Option<&mut FormController> {
        self.overlays.iter_mut().find_map(|o| match o {
            Overlay::Float(_, _, Some(FloatContent::Form(c))) => Some(c),
            _ => None,
        })
    }

    /// Opens `float` as a content-bearing popover, like [`Self::show_popover`],
    /// but with an arbitrary `content: LayoutNode` subtree instead of a
    /// single [`quadraui::Form`] — laid out and painted into
    /// [`quadraui::FloatLayout::content_bounds`] through
    /// [`crate::render::paint_layout`], the same function a binding's own
    /// render-callback path uses for its main tree, every frame this
    /// popover is open. `float.focusable` is forced to `true` regardless
    /// of what the caller set, same reasoning as [`Self::show_popover`].
    /// Replaces (and pops) any float already open.
    ///
    /// Every leaf in `content` (at any nesting depth — directly, or
    /// wrapped in `Row`/`Column`/`Split`/`Tabs`/`Panel`/`SidebarPanel`/
    /// `SplitTree`/`Scroll`/`Styled`) receives focus, click and keyboard
    /// routing exactly as the same leaf would in the main tree: a click
    /// on a `Button` emits the normal
    /// [`crate::semantic::button_clicked_event`]; a click or focused key
    /// on a `Tree`/`List`/`DataTable`/`TabBar`/`TextDisplay`/`Switch`/…
    /// leaf resolves through the same
    /// [`crate::semantic::semantic_event_for_hit`]/
    /// [`crate::semantic::focus_scoped_key_event`] pair
    /// [`Self::handle`] uses for the main tree; typing/toggling a
    /// `"form"` field emits the normal `UiEvent::Form(leaf_id, ..)`
    /// event. Tab/Shift-Tab cycle among the subtree's own stops and
    /// never leave the popover (see [`Self::tab_stops`]'s own doc for
    /// the other half of that cycle). A `MouseDown` landing outside
    /// `float`'s own bounds dismisses it, same as [`Self::show_popover`];
    /// one landing inside is routed into `content` and never reaches the
    /// tree painted underneath.
    pub fn show_popover_layout(
        &mut self,
        mut float: Float,
        measure: FloatMeasure,
        content: LayoutNode,
        backend: &mut dyn Backend,
    ) {
        float.focusable = true;
        self.close_float(backend);
        let layout = float.layout(viewport_rect(backend), measure);
        backend
            .modal_stack_handle()
            .borrow_mut()
            .push_float(&float, layout.bounds);
        self.push_overlay(Overlay::Float(
            float,
            measure,
            Some(FloatContent::Layout(LayoutContent::new(content))),
        ));
    }

    /// Replaces an open `Layout`-content popover's subtree with `content`,
    /// keeping its per-`"form"`-leaf edit state (cursor, selection,
    /// scroll) alive — the "app owns the data, controller owns the edits"
    /// contract [`Self::popover_mut`] documents, for a popover whose
    /// content is a whole subtree rather than a single
    /// [`quadraui::Form`]. Returns whether one was open (`false` for a
    /// plain float, a `Form`-content popover, or no float at all).
    pub fn set_popover_content(&mut self, content: LayoutNode) -> bool {
        match self.overlays.iter_mut().find_map(|o| match o {
            Overlay::Float(_, _, Some(FloatContent::Layout(c))) => Some(c),
            _ => None,
        }) {
            Some(embedded) => {
                embedded.node = content;
                true
            }
            None => false,
        }
    }

    /// Whether a dialog opened via [`Self::open_layout_dialog`] is
    /// currently open.
    pub fn layout_dialog_open(&self) -> bool {
        self.overlays
            .iter()
            .any(|o| matches!(o, Overlay::LayoutDialog(_)))
    }

    /// Opens `dialog` as a modal whose body hosts an arbitrary
    /// `content: LayoutNode` subtree — the dialog's own title/body-text/
    /// button chrome painted by [`Backend::draw_dialog`] as usual, with
    /// `content` laid out and painted into the slot reserved beneath the
    /// body text through [`crate::render::paint_layout`], the same
    /// function a binding's own render-callback path uses for its main
    /// tree. `size` requests the content slot's own width/height; see
    /// [`DialogContentSize`] for each axis's default.
    ///
    /// Every leaf in `content` receives focus, click and keyboard routing
    /// exactly as it would in the main tree — see
    /// [`Self::show_popover_layout`]'s own doc, which describes the same
    /// embedding. Tab/Shift-Tab cycle among the subtree's own stops *and*
    /// the dialog's buttons, and never leave the modal; Enter or Space on
    /// a focused button, a click on one, and an Escape the subtree had no
    /// use for all close the dialog and emit
    /// [`UiEvent::MessageDialogResolved`] with that button's id (Escape
    /// resolving to [`quadraui::Dialog::cancel_button_id`], falling back
    /// to the default and then the last button). Every other input event
    /// is swallowed by the modal; window-lifecycle and `User(..)` events
    /// keep reaching the app's own handler. Replaces (and pops) any
    /// dialog already open, of any kind.
    pub fn open_layout_dialog(
        &mut self,
        dialog: Dialog,
        content: LayoutNode,
        size: DialogContentSize,
        backend: &mut dyn Backend,
    ) {
        self.close_layout_dialog(backend);
        self.close_form_dialog(backend);
        self.close_message_dialog(backend);
        if self.menu_system.borrow().is_open() {
            self.menu_system.borrow_mut().close(backend);
        }
        if self.context_menu_open() {
            self.close_context_menu(backend);
        }
        let embedded = LayoutDialog {
            dialog,
            content: LayoutContent::new(content),
            size,
            button_stops: RefCell::new(Vec::new()),
        };
        let id = embedded.dialog.id;
        let bounds = embedded.layout(backend).bounds;
        backend.modal_stack_handle().borrow_mut().push(id, bounds);
        self.push_overlay(Overlay::LayoutDialog(embedded));
    }

    /// Closes the open layout dialog (if any), popping its
    /// [`quadraui::ModalStack`] entry. Returns whether one was open.
    pub fn close_layout_dialog(&mut self, backend: &mut dyn Backend) -> bool {
        match self.remove_overlay(|o| matches!(o, Overlay::LayoutDialog(_))) {
            Some(Overlay::LayoutDialog(embedded)) => {
                backend
                    .modal_stack_handle()
                    .borrow_mut()
                    .pop(&embedded.dialog.id);
                release_content_focus(backend, &embedded.content);
                true
            }
            _ => false,
        }
    }

    /// Replaces an open layout dialog's subtree with `content`, keeping
    /// its per-`"form"`-leaf edit state alive — the dialog counterpart of
    /// [`Self::set_popover_content`]. Returns whether one was open.
    pub fn set_layout_dialog_content(&mut self, content: LayoutNode) -> bool {
        match self.overlays.iter_mut().find_map(|o| match o {
            Overlay::LayoutDialog(embedded) => Some(embedded),
            _ => None,
        }) {
            Some(embedded) => {
                embedded.content.node = content;
                true
            }
            None => false,
        }
    }

    /// Closes the open float (if any), popping its
    /// [`quadraui::ModalStack`] entry. Returns whether one was open.
    pub fn dismiss_float(&mut self, backend: &mut dyn Backend) -> bool {
        self.close_float(backend)
    }

    fn close_float(&mut self, backend: &mut dyn Backend) -> bool {
        match self.remove_overlay(|o| matches!(o, Overlay::Float(..))) {
            Some(Overlay::Float(float, _, content)) => {
                backend.modal_stack_handle().borrow_mut().pop(&float.id);
                if let Some(FloatContent::Layout(embedded)) = &content {
                    release_content_focus(backend, embedded);
                }
                true
            }
            _ => false,
        }
    }

    fn float_layout(
        &self,
        backend: &dyn Backend,
        float: &Float,
        measure: FloatMeasure,
    ) -> FloatLayout {
        float.layout(viewport_rect(backend), measure)
    }

    // ── RichTextPopup ────────────────────────────────────────────────

    /// Shows `popup` anchored at `anchor` (typically the top-left of the
    /// editor cell it describes, in the backend's ABSOLUTE frame).
    /// Replaces any popup already open.
    pub fn show_rich_text_popup(&mut self, popup: RichTextPopup, anchor: Point) {
        self.push_overlay(Overlay::RichTextPopup(popup, anchor));
    }

    /// Closes the open rich-text popup (if any) without reporting a link
    /// click. Returns whether one was open.
    pub fn dismiss_rich_text_popup(&mut self) -> bool {
        self.remove_overlay(|o| matches!(o, Overlay::RichTextPopup(..)))
            .is_some()
    }

    /// The open rich-text popup (if any) and the anchor it was shown
    /// against, for [`Self::handle`]'s hit-test dispatch — see
    /// [`Self::find_completions`]'s own doc for why this clones out of
    /// [`Self::overlays`] rather than borrowing it.
    fn find_rich_text_popup(&self) -> Option<(RichTextPopup, Point)> {
        self.overlays.iter().find_map(|o| match o {
            Overlay::RichTextPopup(p, a) => Some((p.clone(), *a)),
            _ => None,
        })
    }

    fn rich_text_popup_layout(
        &self,
        backend: &dyn Backend,
        popup: &RichTextPopup,
        anchor: Point,
    ) -> RichTextPopupLayout {
        let m = backend.measure();
        let measure = RichTextPopupMeasure::new(m.char_width * 30.0, m.line_height)
            .with_scale_rows(backend.scales_text_rows());
        popup.layout(
            anchor.x,
            anchor.y,
            viewport_rect(backend),
            measure,
            |_, start, end| m.char_width * (end - start) as f32,
        )
    }

    /// The single render pass over [`Self::overlays`], in z-order
    /// (lowest [`Overlay::rank`] — bottommost — first, toast topmost of
    /// all): the one call that replaces the seven kind-specific
    /// `render_*` methods this engine used to expose
    /// (`render_ctx_menu`/`render_dialog`/`render_toast`/`render_tooltip`/
    /// `render_completions`/`render_float`/`render_rich_text_popup`), each
    /// painted at its own fixed spot in `host::render_overlays`'s old call
    /// sequence. Call once per render, after the binding's own paint pass
    /// and [`InteractionEngine::sync_menu_bar`]/[`InteractionEngine::sync_forms`]/
    /// [`InteractionEngine::sync_text_input_carets`].
    pub fn render_overlays(&self, backend: &mut dyn Backend) {
        for overlay in &self.overlays {
            match overlay {
                Overlay::ContextMenu(controller) => controller.render(backend),
                Overlay::Float(float, measure, content) => {
                    let layout = self.float_layout(backend, float, *measure);
                    backend.draw_float(float, &layout);
                    match content {
                        Some(FloatContent::Form(controller)) => {
                            controller.render(backend, layout.content_bounds);
                        }
                        Some(FloatContent::Layout(content)) => {
                            content.render(backend, layout.content_bounds, &[]);
                        }
                        None => {}
                    }
                }
                Overlay::RichTextPopup(popup, anchor) => {
                    let layout = self.rich_text_popup_layout(backend, popup, *anchor);
                    backend.draw_rich_text_popup(popup, &layout);
                }
                Overlay::Completions(completions, cursor) => {
                    let layout = self.completions_layout(backend, completions, *cursor);
                    backend.draw_completions(completions, &layout);
                }
                Overlay::Dialog(dialog) => dialog.render(backend),
                Overlay::FormDialog(_, dialog) => dialog.render(backend),
                Overlay::LayoutDialog(embedded) => embedded.render(backend),
                Overlay::Tooltip(tooltip, anchor) => {
                    let layout = self.tooltip_layout(backend, tooltip, *anchor);
                    backend.draw_tooltip(tooltip, &layout);
                }
                // A toast stack paints as one group, after this loop —
                // see the call below.
                Overlay::Toast(_) => {}
            }
        }
        if self.overlays.iter().any(|o| matches!(o, Overlay::Toast(_))) {
            let overlay = self.toast_overlay();
            backend.draw_toast_overlay(viewport_rect(backend), &overlay);
        }
    }

    /// Derives this frame's tab stops directly from the hit map
    /// [`InteractionEngine::set_hits`] last recorded, in paint order.
    /// `Split`/`SplitTree`/`Panel`/`SidebarPanel` are chrome around other
    /// content, not focusable leaves themselves, so they are skipped (their
    /// own action/toolbar buttons aren't recorded as separate `PaintedHit`
    /// entries — see `crate::hit::HitDetail::PanelAction` et al. — so a
    /// future keyboard-nav pass over those specific buttons still has
    /// nothing to pick up here); `Extra` is a tab stop only when its
    /// descriptor opts in via
    /// [`crate::extra_paint::ExtraPaint::tab_stop`] (currently `Switch` and
    /// `TextInput`); `TextDisplay` is always one (scroll/copy need
    /// keyboard focus the same way a `Tree`/`List` does); every other
    /// variant yields its id and bounding `Rect`.
    ///
    /// Returns empty while an [`Overlay::FormDialog`] or a Form-content
    /// [`Overlay::Float`] (a popover opened via [`Self::show_popover`]) is
    /// open, regardless of what the main tree itself painted this frame —
    /// both trap Tab inside their own embedded form internally (see
    /// [`FormDialogController`]'s "Keyboard model" doc section and
    /// [`Self::dispatch_overlays`]'s popover handling), so the main tree
    /// must report no stops at all while either is open, or the global
    /// cycle below would reach straight through the modal to whatever the
    /// host painted underneath it. A `Layout`-content overlay — a popover
    /// opened via [`Self::show_popover_layout`], or a dialog opened via
    /// [`Self::open_layout_dialog`] — instead reports *its own* content's
    /// stops here (plus, for the dialog, its own buttons, so Tab reaches
    /// "OK"/"Cancel" without leaving the modal): unlike the two
    /// self-contained cases above, nothing else traps Tab for it, so this
    /// is the only place that can.
    /// `quadraui::runtime::preprocess_event` claims Tab/Shift+Tab
    /// unconditionally — cycling focus among whichever stops this
    /// returns, via `backend`'s own focus manager — whenever this is
    /// non-empty, *before* the key ever reaches [`Self::handle`].
    ///
    /// Excludes a main-tree `"form"` extra's own stop while
    /// [`Self::form_engine_focus`] names it (i.e. `backend`'s focus
    /// manager named it as of the last [`Self::sync_forms`] call) — Tab
    /// inside it is then [`Self::handle_form_tab`]'s job (move between
    /// fields, then leave the form once a cycle at either end has
    /// nowhere left to go inside it), not the generic cross-widget cycle
    /// above. Unlike the `FormDialog`/popover case above, this removes
    /// only that one entry rather than the whole list — every *other*
    /// sibling stop stays reachable, and the form itself still appears
    /// here (so Tab can reach it in the first place) until engine focus
    /// actually lands on it.
    pub fn tab_stops(&self) -> Vec<(WidgetId, Rect)> {
        if self.overlays.iter().any(|o| {
            matches!(
                o,
                Overlay::FormDialog(..) | Overlay::Float(_, _, Some(FloatContent::Form(_)))
            )
        }) {
            return Vec::new();
        }
        if let Some(embedded) = self.overlays.iter().find_map(|o| match o {
            Overlay::LayoutDialog(embedded) => Some(embedded),
            _ => None,
        }) {
            let mut stops = embedded.content.tab_stops();
            stops.extend(embedded.button_stops.borrow().iter().copied());
            return stops;
        }
        if let Some(content) = self.overlays.iter().find_map(|o| match o {
            Overlay::Float(_, _, Some(FloatContent::Layout(content))) => Some(content),
            _ => None,
        }) {
            return content.tab_stops();
        }
        let mut stops = tab_stops_from_hits(&self.hit_map.borrow());
        let form_engine_focus = self.form_engine_focus.borrow();
        stops.retain(|(id, _)| !form_engine_focus.contains(id));
        stops
    }

    /// If `id` is a direct child of a `Scroll` (per
    /// [`PaintedHit::Scroll::children`]/[`enclosing_scroll_child`]) that
    /// isn't already fully inside that `Scroll`'s own visible range,
    /// scrolls it there and persists the new offset via
    /// [`apply_scroll_offset`], then dispatches the resulting
    /// `UiEvent::ScrollOffsetChanged` through `sink` — the shared "a
    /// focused child should be fully visible" side effect, called both
    /// from [`Self::handle`]'s own click-to-focus bookkeeping and for a
    /// directly-dispatched `UiEvent::FocusChanged` (e.g. a binding's own
    /// Tab-cycling, which calls `AppLogic::handle` straight through to
    /// [`Self::handle`] without going through this engine's click-to-focus
    /// code at all). Every other offset-changing path (wheel, page keys,
    /// thumb drag, track paging) dispatches this same event, in both
    /// controlled and uncontrolled mode — see [`crate::widget_state`]'s own
    /// module doc — and this is no exception, even though `id` landing
    /// here is itself a side effect of a focus change rather than a direct
    /// scroll gesture. Returns `Reaction::Continue` when nothing scrolled,
    /// so a caller can fold the result into its own `Reaction` via `merge`
    /// either way.
    fn scroll_focused_child_into_view(
        &self,
        id: WidgetId,
        sink: &mut dyn EventSink,
    ) -> quadraui::Reaction {
        let Some((scroll_id, start, len)) = enclosing_scroll_child(&self.hit_map.borrow(), id)
        else {
            return quadraui::Reaction::Continue;
        };
        let Some(view) = scroll_view_at(&self.hit_map.borrow(), scroll_id) else {
            return quadraui::Reaction::Continue;
        };
        let Some(ScrollViewEvent::Scrolled { offset: new_offset }) =
            scroll_child_into_view(&view, start, len)
        else {
            return quadraui::Reaction::Continue;
        };
        apply_scroll_offset(&self.widget_state, scroll_id, new_offset);
        quadraui::Reaction::Redraw.merge(sink.dispatch(
            &UiEvent::ScrollOffsetChanged {
                widget: scroll_id,
                new_offset: new_offset.round() as usize,
            },
            None,
        ))
    }

    /// The hover/pressed [`InteractionState`] for the chrome-bar extra
    /// painted under id `bar_id` this frame, read from [`Self::chrome_hover`]/
    /// [`Self::chrome_pressed`] — call from `render`, once per chrome-bar
    /// extra, right before repainting it, so it shows real hover/pressed
    /// state instead of [`InteractionState::default()`].
    pub fn chrome_interaction(&self, bar_id: WidgetId) -> InteractionState {
        InteractionState::from_parts(
            self.chrome_hover.borrow().get(&bar_id).copied(),
            self.chrome_pressed.borrow().get(&bar_id).copied(),
        )
    }

    /// Rebuilds [`Self::menu_system`]'s menu list from this frame's `"menu_bar"`
    /// extra (if any) and paints its dropdown/submenu popups on top of the
    /// plain bar `hits` already recorded, then remembers the bar's absolute
    /// bounds in [`Self::menu_bar_rect`] for [`InteractionEngine::handle`] to
    /// reuse. A no-op (and clears `menu_bar_rect`) when no `"menu_bar"`
    /// extra was painted this frame. Call once per render, after `hits` is
    /// fully built — the extras loop in `draw_bands`/`paint_leaf` already
    /// painted the plain, non-interactive bar via `MenuBar`'s own
    /// `ExtraPaint::paint`; this repaints it with `menu_system`'s own
    /// open/focused state (and the dropdown, when one is open) layered on
    /// top, the same "paint after everything else" contract
    /// `ContextMenuController::render` documents for itself. Only the
    /// *first* `"menu_bar"` extra painted this frame is wired up this
    /// way — a second one in the same frame still paints (via `paint_leaf`'s
    /// normal `Extra` path) but is not interactive.
    ///
    /// `menu_system.render` draws from `MenuSystem`'s own open/focused/id
    /// state (see `MenuSystem::build_menu_bar`), not the descriptor's
    /// `open_item`/`focused_item`/`id` fields — those are overridden, so a
    /// binding that sets them declaratively (e.g. to render a pressed
    /// visual) has no effect once the bar is interactive.
    pub fn sync_menu_bar(&self, backend: &mut dyn Backend, hits: &[PaintedHit]) {
        let found = hits.iter().find_map(|hit| match hit {
            PaintedHit::Extra { area, extra, .. } => {
                extra_as::<MenuBar>(extra).map(|menu_bar| (*area, menu_bar))
            }
            _ => None,
        });
        let Some((area, menu_bar)) = found else {
            // The `"menu_bar"` extra is gone from this frame (the app
            // stopped rendering one, or swapped it for something else).
            // Close any dropdown first so the `"menu-system-dropdown"`
            // entry `MenuSystem::open_menu` pushed onto the modal stack
            // gets popped — otherwise it leaks, and the dropdown would
            // silently reappear (with nothing to hit-test it) if the
            // extra ever comes back.
            if self.menu_system.borrow().is_open() {
                self.menu_system.borrow_mut().close(backend);
            }
            *self.menu_bar_rect.borrow_mut() = None;
            return;
        };
        let menus: Vec<MenuDef> = menu_bar
            .items
            .iter()
            .map(|item| MenuDef {
                id: item.id,
                label: item.label.clone(),
                disabled: item.disabled,
                items: item.submenu.clone().unwrap_or_default(),
            })
            .collect();
        let mut menu_system = self.menu_system.borrow_mut();
        menu_system.set_menus(menus);
        menu_system.render(backend, area);
        *self.menu_bar_rect.borrow_mut() = Some(area);
    }

    /// Repaints every `"form"` extra painted this frame through its own
    /// cached [`FormController`] (creating one the first time an id is
    /// seen), so the form shows [`FormController`]'s own richer,
    /// stateful rendering (its live scroll offset, in-progress text/date
    /// edit overlay, ...) instead of just [`quadraui::Form`]'s own
    /// plain, non-interactive [`crate::extra_paint::ExtraPaint::paint`] —
    /// the same "paint after everything else, from the stateful
    /// controller instead of the bare descriptor" pattern
    /// [`Self::sync_menu_bar`] already uses for `"menu_bar"`. Call once
    /// per render, after the extras loop in `draw_bands`/`paint_leaf`
    /// already painted the plain bar. Unlike `sync_menu_bar`'s
    /// single `"menu_bar"` instance, every `"form"` extra painted this
    /// frame gets its own entry, keyed by its own id — a `"form"` is not
    /// a singleton overlay the way `"menu_bar"`/`"find_replace"` are.
    /// A cached entry whose id wasn't painted this frame is dropped, so a
    /// form that stops rendering doesn't keep answering events forever.
    ///
    /// Seeds a newly-created controller's own `scroll_offset` from the
    /// descriptor's `Form.scroll_offset` the first time its id is seen, so
    /// a host that paints a non-zero `scroll_offset` on insert (the
    /// contract the plain, non-controller `ExtraPaint::paint` path already
    /// honours) doesn't silently lose it to `FormController::new`'s `0`
    /// the moment the form gets wired through this cache. Only on
    /// creation, not every frame: from then on `FormController` owns
    /// `scroll_offset` itself (wheel/thumb-drag advance it internally, the
    /// same "host doesn't have to echo it back" contract
    /// `compose::form_controller`'s own module doc describes for the
    /// cursor) — reseeding it from the descriptor on every frame would
    /// snap a live scroll straight back to whatever stale value a host
    /// that never echoes it keeps sending.
    ///
    /// `has_focus`, by contrast, IS reconciled every frame: the
    /// descriptor's own `Form.has_focus` OR'd with whether `backend`'s
    /// focus manager currently names this form. Unlike `scroll_offset`,
    /// engine-level focus is already an authoritative value this engine
    /// tracks continuously, so ORing it in is never destructive of a
    /// legitimate internal source of truth the way reseeding
    /// `scroll_offset` would be — and it's what makes the focused-row
    /// highlight track engine focus in both directions: appearing the
    /// very next render after a click moves focus onto the form (not just
    /// after the first keystroke, the only other site that calls
    /// `set_has_focus`), and clearing once focus moves elsewhere even if
    /// the host keeps echoing a stale `focused_field`/`has_focus: true`.
    pub fn sync_forms(&self, backend: &mut dyn Backend, hits: &[PaintedHit]) {
        let mut seen = Vec::new();
        for hit in hits {
            let PaintedHit::Extra {
                id, area, extra, ..
            } = hit
            else {
                continue;
            };
            let Some(form) = extra_as::<Form>(extra) else {
                continue;
            };
            seen.push(*id);
            let engine_focused = backend.focus_manager().focused() == Some(id);
            if engine_focused {
                self.form_engine_focus.borrow_mut().insert(*id);
            } else {
                self.form_engine_focus.borrow_mut().remove(id);
            }
            // `Form.focused_field`'s own controlled/uncontrolled
            // resolution — see that field's doc. `None` (uncontrolled)
            // resumes from whatever `Self::handle`'s own Tab/Shift-Tab/
            // Up/Down/click routing last persisted for this form id (or
            // stays `None` the first time it's seen); `Some(id)`
            // (controlled) wins outright and is written back so a later
            // uncontrolled frame resumes from it instead of an older
            // engine-owned value.
            let mut form = form.clone();
            form.focused_field = resolve_form_focused_field(&self.widget_state, &form);
            let mut controllers = self.form_controllers.borrow_mut();
            let is_new = !controllers.contains_key(id);
            let controller = controllers
                .entry(*id)
                .or_insert_with(|| FormController::new(id.as_str().to_string()));
            controller.set_form(form.clone());
            if is_new {
                controller.set_scroll_offset(form.scroll_offset);
            }
            controller.set_has_focus(form.has_focus || engine_focused);
            controller.render_and_cache(backend, *area);
            self.form_rects.borrow_mut().insert(*id, *area);
        }
        self.form_controllers
            .borrow_mut()
            .retain(|id, _| seen.contains(id));
        self.form_rects
            .borrow_mut()
            .retain(|id, _| seen.contains(id));
        self.form_engine_focus
            .borrow_mut()
            .retain(|id| seen.contains(id));
        *self.form_order.borrow_mut() = seen;
    }

    /// Tab/Shift-Tab inside a main-tree `"form"` extra that currently
    /// holds engine focus — called from [`Self::handle`]'s own
    /// `KeyPressed` routing once [`Self::tab_stops`]'s own exclusion (see
    /// its doc) has let the key reach here instead of the generic
    /// cross-widget cycle. `forward` is `true` for `Tab`, `false` for
    /// `Shift+Tab`/`BackTab`.
    ///
    /// Not at either end of [`quadraui::primitives::form::focus_order`]:
    /// forwards a synthesized `Tab`/`BackTab` into `form_id`'s own cached
    /// [`FormController`], exactly as [`Self::handle`] already did for
    /// every other `KeyPressed` reaching a focused form — its own
    /// `move_focus` reseeds the new field's text/date-edit cursor and
    /// scrolls it into view, which reimplementing the move here from
    /// scratch would lose. At either end with another app-level stop to
    /// leave to: hands engine focus to the next/previous one instead
    /// (mirrors
    /// [`quadraui::compose::form_dialog::FormDialogController::navigate`]'s
    /// own boundary check, which hands focus to its button row at the
    /// same point) — "leaving the form" the same way Tab already leaves
    /// any other single-stop widget. At either end with nowhere to leave
    /// to (the form is this frame's only stop): forwards the key the same
    /// way the non-boundary case does, so `move_focus`'s own wraparound
    /// moves focus to the opposite end instead of going dead. `None` when
    /// `form_id` names no cached controller at all (nothing to do; the
    /// caller's own `KeyPressed` should keep flowing through
    /// [`Self::handle`] unchanged).
    fn handle_form_tab(
        &self,
        form_id: WidgetId,
        rect: Rect,
        forward: bool,
        backend: &mut dyn Backend,
        sink: &mut dyn EventSink,
    ) -> Option<FormTabOutcome> {
        let form = self
            .form_controllers
            .borrow()
            .get(&form_id)
            .and_then(|c| c.form().cloned())?;
        let stops = focus_order(&form);
        let delta: isize = if forward { 1 } else { -1 };
        if form_focus_at_boundary(&form, &stops, delta) {
            if let Some(next_id) = self.next_tab_stop_leaving(form_id, forward) {
                let mut focus_reaction = quadraui::Reaction::Continue;
                if backend.set_focus(Some(next_id)) {
                    focus_reaction = sink.dispatch(&UiEvent::FocusChanged(Some(next_id)), None);
                    if matches!(focus_reaction, quadraui::Reaction::Exit) {
                        return Some(FormTabOutcome::LeftForm(focus_reaction));
                    }
                    focus_reaction =
                        focus_reaction.merge(self.scroll_focused_child_into_view(next_id, sink));
                    if matches!(focus_reaction, quadraui::Reaction::Exit) {
                        return Some(FormTabOutcome::LeftForm(focus_reaction));
                    }
                }
                return Some(FormTabOutcome::LeftForm(
                    quadraui::Reaction::Redraw.merge(focus_reaction),
                ));
            }
            // Nowhere to leave to — the form is this frame's only tab
            // stop, so falls through to the same key-forwarding below the
            // non-boundary case uses. `FormController::move_focus`'s own
            // `cycle_index` wraps `Some(i)` at either end back to the
            // opposite one, so this still moves rather than going dead.
        }
        let key_event = UiEvent::KeyPressed {
            key: Key::Named(if forward {
                NamedKey::Tab
            } else {
                NamedKey::BackTab
            }),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let mut controllers = self.form_controllers.borrow_mut();
        let controller = controllers.get_mut(&form_id)?;
        controller.set_has_focus(true);
        match controller.handle_event(&key_event, backend, rect) {
            Handled::Event(FormControllerAction::FormAction(form_event)) => {
                if let FormEvent::FocusChanged { id } = &form_event {
                    apply_form_focus_changed(&self.widget_state, form_id, *id);
                }
                Some(FormTabOutcome::Moved(form_event))
            }
            _ => Some(FormTabOutcome::PassThrough),
        }
    }

    /// The next (`forward`)/previous app-level tab stop after `form_id`
    /// in this frame's own full [`tab_stops_from_hits`] order — the
    /// "leave the form" half of [`Self::handle_form_tab`]. `None` when
    /// `form_id` isn't one of this frame's stops (it was removed from
    /// [`Self::hit_map`] between render and this keystroke) or it is the
    /// only one (nothing to leave to).
    fn next_tab_stop_leaving(&self, form_id: WidgetId, forward: bool) -> Option<WidgetId> {
        let stops = tab_stops_from_hits(&self.hit_map.borrow());
        if stops.len() < 2 {
            return None;
        }
        let idx = stops.iter().position(|(id, _)| *id == form_id)?;
        let next_idx = quadraui::focus::cycle_index(idx, if forward { 1 } else { -1 }, stops.len());
        Some(stops[next_idx].0)
    }

    /// Up/Down field-to-field navigation for a focused main-tree `"form"`
    /// — called from [`Self::handle`]'s own `KeyPressed` routing only
    /// once `focused`'s own [`FormController::handle_event`] has already
    /// reported [`Handled::Ignored`] for the identical keypress, meaning
    /// the focused field itself has no use for Up/Down (a
    /// `DatePicker`/`ComboBox`/`Dropdown`/`NumberInput`/`RadioGroup`
    /// already claims both internally and never reaches here — trying the field first
    /// and falling back to this instead of duplicating `FormController`'s
    /// own per-kind knowledge of which keys each consumes). Clamps at
    /// either end rather than wrapping or leaving the form — unlike
    /// Tab/Shift-Tab, Up/Down has no "leave the form" contract.
    fn maybe_move_form_focus_vertically(
        &self,
        form_id: WidgetId,
        event: &UiEvent,
    ) -> Option<FormEvent> {
        let UiEvent::KeyPressed {
            key: Key::Named(named @ (NamedKey::Up | NamedKey::Down)),
            modifiers,
            ..
        } = event
        else {
            return None;
        };
        if modifiers.ctrl || modifiers.alt || modifiers.cmd {
            return None;
        }
        let form = self
            .form_controllers
            .borrow()
            .get(&form_id)
            .and_then(|c| c.form().cloned())?;
        let stops = focus_order(&form);
        if stops.is_empty() {
            return None;
        }
        let delta: isize = if matches!(named, NamedKey::Down) {
            1
        } else {
            -1
        };
        let cur = form
            .focused_field
            .and_then(|id| stops.iter().position(|s| *s == id));
        let next_idx = match cur {
            Some(i) => {
                let next = i as isize + delta;
                if next < 0 || next as usize >= stops.len() {
                    return None;
                }
                next as usize
            }
            None if delta > 0 => 0,
            None => stops.len() - 1,
        };
        let id = stops[next_idx];
        apply_form_focus_changed(&self.widget_state, form_id, id);
        Some(FormEvent::FocusChanged { id })
    }

    /// Repaints every standalone `"text_input"` extra this frame whose
    /// cached [`quadraui::TextEditor`] (seeded by [`text_input_click_event`]/
    /// [`text_input_insert_event`]) disagrees with the descriptor's own
    /// `cursor_line`/`cursor_col` — the same "paint after everything else,
    /// from the richer live state" overlay [`Self::sync_forms`] uses for
    /// `"form"`. Without this, `text_input_click_event`'s `SetCursor`
    /// moves the caret only *logically*: the next keystroke starts from
    /// the clicked position, but nothing ever paints it there, since
    /// [`crate::extra_paint::ExtraPaint::paint`] for `TextInput` draws the
    /// raw descriptor — whose `cursor_col` is normally `0`, because
    /// `FormEvent::TextInputChanged`/`Committed` carry no cursor for a
    /// host to echo back onto it. Call once per render, after the extras
    /// loop already painted the plain input and after
    /// [`Self::sync_forms`] (order between the two doesn't matter — they
    /// never touch the same extra kind).
    pub fn sync_text_input_carets(&self, backend: &mut dyn Backend, hits: &[PaintedHit]) {
        let editors = self.text_editors.borrow();
        for hit in hits {
            let PaintedHit::Extra {
                id, area, extra, ..
            } = hit
            else {
                continue;
            };
            let Some(text_input) = extra_as::<TextInput>(extra) else {
                continue;
            };
            let Some(editor) = editors.get(id) else {
                continue;
            };
            let live = (editor.input.cursor_line, editor.input.cursor_col);
            if live == (text_input.cursor_line, text_input.cursor_col) {
                continue;
            }
            let mut repainted = text_input.clone();
            repainted.cursor_line = live.0;
            repainted.cursor_col = live.1;
            backend.draw_text_input(*area, &repainted);
        }
    }

    /// The single hit-test/dispatch pass over [`Self::overlays`],
    /// collapsing what used to be five hand-written per-kind blocks in
    /// [`Self::handle`] (menu-bar/context-menu routing stays separate —
    /// see that method's own doc for why): walks the stack top-to-bottom
    /// (descending [`Overlay::rank`], matching [`Self::render_overlays`]'s
    /// ascending paint order in reverse) and gives each kind a chance to
    /// claim `event` with its own nuance — toast claims a `MouseDown`
    /// landing on its own dismiss/action region only, dialog claims any
    /// event, completions/rich-text-popup each claim Escape and a
    /// `MouseDown`, float claims Escape only when focusable. The FIRST
    /// kind that claims it wins; `None` means nothing in the stack
    /// claimed `event`, so [`Self::handle`]'s own routing (menu-bar,
    /// click-to-focus, ...) continues exactly as if none of this existed.
    /// Toast is always topmost by [`Overlay::rank`], so it is checked
    /// first regardless of what else is open.
    fn dispatch_overlays(
        &mut self,
        event: &UiEvent,
        backend: &mut dyn Backend,
        sink: &mut dyn EventSink,
    ) -> Option<quadraui::Reaction> {
        use quadraui::Reaction;

        if self.overlays.iter().any(|o| matches!(o, Overlay::Toast(_))) {
            if let UiEvent::MouseDown { position, .. } = event {
                match self.toast_layout(backend).hit_test(position.x, position.y) {
                    ToastHit::Dismiss(id) => {
                        self.dismiss_toast(id);
                        return Some(Reaction::Redraw);
                    }
                    ToastHit::Action(action_id) => {
                        return Some(
                            Reaction::Redraw.merge(
                                sink.dispatch(&UiEvent::ToastActionClicked(action_id), None),
                            ),
                        );
                    }
                    ToastHit::Body(_) => return Some(Reaction::Continue),
                    ToastHit::Empty => {}
                }
            }
        }

        // `MessageDialogController::handle_event` reports `Handled::Ignored`
        // immediately when nothing is open, so this is safe to run
        // unconditionally.
        if let Some(dialog) = self.overlays.iter_mut().find_map(|o| match o {
            Overlay::Dialog(d) => Some(d),
            _ => None,
        }) {
            match dialog.handle_event(event, &*backend) {
                Handled::Event(choice) => {
                    // The dialog is closing regardless of what `sink`
                    // itself returns — force the repaint that takes it
                    // off screen, same as the context-menu's own
                    // `ContextMenuOutcome::Event` arm in `Self::handle`.
                    self.close_message_dialog(backend);
                    return Some(
                        Reaction::Redraw
                            .merge(sink.dispatch(&UiEvent::MessageDialogResolved(choice), None)),
                    );
                }
                Handled::Consumed => return Some(Reaction::Redraw),
                Handled::Ignored => {}
            }
        }

        // `FormDialogController::handle_event` is also self-contained —
        // typing a field, toggling a `FieldKind::Toggle`, Tab/Shift+Tab
        // between fields and buttons, Enter/Escape on a button, and a
        // click anywhere in the dialog are all resolved internally (see
        // that type's own "Keyboard model" doc section). It returns
        // `Handled::Ignored` for an event it has no use for — e.g. a
        // `CharTyped`/arrow key while a button (not a field) holds
        // keyboard focus — which falls through below, same pre-existing
        // gap `Overlay::Dialog`'s own `MessageDialogController` already
        // has.
        if let Some((dialog_id, dialog)) = self.overlays.iter_mut().find_map(|o| match o {
            Overlay::FormDialog(id, d) => Some((*id, d)),
            _ => None,
        }) {
            match dialog.handle_event(event, backend) {
                Handled::Event(FormDialogAction::ButtonClicked(id)) => {
                    // Closing regardless of what `sink` itself returns —
                    // same "force the repaint" reasoning as the message
                    // dialog's own `Handled::Event` arm above.
                    self.close_form_dialog(backend);
                    return Some(
                        Reaction::Redraw
                            .merge(sink.dispatch(&UiEvent::MessageDialogResolved(id), None)),
                    );
                }
                Handled::Event(FormDialogAction::FormAction(form_event)) => {
                    return Some(
                        Reaction::Redraw
                            .merge(sink.dispatch(&UiEvent::Form(dialog_id, form_event), None)),
                    );
                }
                Handled::Consumed => return Some(Reaction::Redraw),
                Handled::Ignored => {}
            }
        }

        // A layout dialog is modal the same way the two dialogs above
        // are: it claims every input event, routing it into its own
        // content subtree (see `dispatch_layout_dialog`'s own doc for the
        // precedence). `LayoutDialogOutcome::Ignored` — every *non*-input
        // event, from `WindowResized` to an async `User(..)` payload —
        // falls through here (`{}`, not an early `return`) all the way to
        // `Self::handle`'s own final `sink.dispatch`, so an app with a
        // modal up still sees its window get resized or closed.
        let layout_dialog_outcome = self
            .overlays
            .iter()
            .find_map(|o| match o {
                Overlay::LayoutDialog(embedded) => Some(embedded),
                _ => None,
            })
            .map(|embedded| {
                dispatch_layout_dialog(embedded, event, backend, sink, &self.text_editors)
            });
        match layout_dialog_outcome {
            Some(LayoutDialogOutcome::Resolved(id)) => {
                // Closing regardless of what `sink` itself returns —
                // same "force the repaint" reasoning as the message
                // dialog's own `Handled::Event` arm above.
                self.close_layout_dialog(backend);
                return Some(
                    Reaction::Redraw
                        .merge(sink.dispatch(&UiEvent::MessageDialogResolved(id), None)),
                );
            }
            Some(LayoutDialogOutcome::Claimed(reaction)) => return Some(reaction),
            Some(LayoutDialogOutcome::Ignored) | None => {}
        }

        if let Some((completions, cursor)) = self.find_completions() {
            match event {
                UiEvent::KeyPressed {
                    key: Key::Named(NamedKey::Escape),
                    ..
                } => {
                    self.dismiss_completions();
                    return Some(Reaction::Redraw);
                }
                UiEvent::MouseDown { position, .. } => {
                    let layout = self.completions_layout(backend, &completions, cursor);
                    match layout.hit_test(position.x, position.y) {
                        CompletionsHit::Item(idx) => {
                            self.dismiss_completions();
                            return Some(Reaction::Redraw.merge(sink.dispatch(
                                &UiEvent::CompletionItemActivated(completions.id, idx),
                                None,
                            )));
                        }
                        CompletionsHit::Inert => return Some(Reaction::Continue),
                        CompletionsHit::Empty => {
                            self.dismiss_completions();
                            return Some(Reaction::Redraw);
                        }
                    }
                }
                _ => {}
            }
        }

        if let Some((popup, anchor)) = self.find_rich_text_popup() {
            match event {
                UiEvent::KeyPressed {
                    key: Key::Named(NamedKey::Escape),
                    ..
                } => {
                    self.dismiss_rich_text_popup();
                    return Some(Reaction::Redraw);
                }
                UiEvent::MouseDown { position, .. } => {
                    let layout = self.rich_text_popup_layout(backend, &popup, anchor);
                    match layout.hit_test(position.x, position.y) {
                        RichTextPopupHit::Link(idx) => {
                            let url = popup
                                .links
                                .get(idx)
                                .map(|l| l.url.clone())
                                .unwrap_or_default();
                            return Some(Reaction::Redraw.merge(
                                sink.dispatch(&UiEvent::RichTextLinkClicked(popup.id, url), None),
                            ));
                        }
                        RichTextPopupHit::Outside => {
                            self.dismiss_rich_text_popup();
                            return Some(Reaction::Redraw);
                        }
                        RichTextPopupHit::Char(..)
                        | RichTextPopupHit::ScrollbarTrack
                        | RichTextPopupHit::ScrollbarThumb
                        | RichTextPopupHit::Body => return Some(Reaction::Continue),
                    }
                }
                _ => {}
            }
        }

        // A content-bearing float ("popover", opened via
        // `Self::show_popover`) claims a positioned mouse event landing
        // inside its own bounds, and a position-less keyboard/clipboard
        // event (typing, toggling a field, Tab/Shift-Tab, pasting, or
        // Escape) — these route into its embedded `FormController`
        // directly, the same self-contained model `Overlay::FormDialog`
        // above uses. Escape is deliberately routed in here (rather than
        // short-circuited before this block, the way `is_escape` used to)
        // so a focused, open `ComboBox` field gets first claim on it —
        // `FormController::handle_key`'s own combo-box interception
        // closes the popup instead of leaving the whole popover dismissed
        // out from under it, mirroring `FormDialogController`'s own
        // documented Escape precedence. A `MouseDown` landing outside the
        // popover's own bounds dismisses it unconditionally, before any
        // of that routing. Every other position-less `UiEvent`
        // (`WindowResized`, `WindowClose`, `User(..)`, ...) has nothing
        // to do with the popover's own fields, so it must fall through
        // here (`{}`, not an early `return`) all the way to the final
        // `sink.dispatch` in `Self::handle` — an app with a popover open
        // still needs to see its window get resized or closed. A plain
        // chrome-only float (content `None`) still does NOT claim mouse
        // clicks here — its content is painted by the host, not this
        // engine (see `Self::show_float`'s own doc) — so a click anywhere
        // near one of those keeps flowing through the normal hit-map
        // routing below.
        if let Some((float, measure)) = self.overlays.iter().find_map(|o| match o {
            Overlay::Float(f, m, Some(_)) => Some((f.clone(), *m)),
            _ => None,
        }) {
            let layout = self.float_layout(backend, &float, measure);
            let position = event_position(event);
            let inside = position.map(|p| layout.bounds.contains(p));
            if matches!(event, UiEvent::MouseDown { .. }) && inside == Some(false) {
                self.dismiss_float(backend);
                return Some(Reaction::Redraw);
            }
            let traps_without_position = position.is_none()
                && matches!(
                    event,
                    UiEvent::KeyPressed { .. }
                        | UiEvent::CharTyped(..)
                        | UiEvent::ClipboardPaste(..)
                );
            if inside == Some(true) || traps_without_position {
                let layout_content = self.overlays.iter().find_map(|o| match o {
                    Overlay::Float(f, _, Some(FloatContent::Layout(content)))
                        if f.id == float.id =>
                    {
                        Some(content)
                    }
                    _ => None,
                });
                if let Some(content) = layout_content {
                    // `LayoutContent::dispatch`'s own `None` means
                    // "nothing in the subtree claimed this" — an Escape
                    // the popover itself owns, or a key with no meaning
                    // for the focused leaf. A positioned event that
                    // landed inside the popover is still claimed either
                    // way: it visually hit the popover, so the tree
                    // painted underneath must never see it.
                    match content.dispatch(event, backend, sink, &self.text_editors) {
                        Some(result) => return Some(result),
                        None if inside == Some(true) => return Some(Reaction::Continue),
                        None => {}
                    }
                } else {
                    let outcome = self.overlays.iter_mut().find_map(|o| match o {
                        Overlay::Float(f, _, Some(FloatContent::Form(c))) if f.id == float.id => {
                            Some(c.handle_event(event, backend, layout.content_bounds))
                        }
                        _ => None,
                    });
                    match outcome {
                        Some(Handled::Event(FormControllerAction::FormAction(form_event))) => {
                            return Some(
                                Reaction::Redraw.merge(
                                    sink.dispatch(&UiEvent::Form(float.id, form_event), None),
                                ),
                            );
                        }
                        Some(
                            Handled::Event(FormControllerAction::ScrollChanged) | Handled::Consumed,
                        ) => {
                            return Some(Reaction::Redraw);
                        }
                        // Falls through to the Escape-dismiss check just below
                        // (for Escape the `FormController` had no use for —
                        // e.g. no open `ComboBox` to close) and, past that,
                        // all the way to `Self::handle`'s own final
                        // `sink.dispatch` — same as `Overlay::FormDialog`'s
                        // own `Handled::Ignored` arm above.
                        Some(Handled::Ignored) | None => {}
                    }
                }
            }
        }

        // Float does NOT claim mouse clicks here: its content is painted
        // by the host, not this engine (see `Self::show_float`'s doc), so
        // a click anywhere near it has to keep flowing through the
        // normal hit-map routing below for that content to ever receive
        // it. A non-focusable hint float never steals Escape either, same
        // distinction [`quadraui::ModalStack::top_focusable`]'s own doc
        // draws.
        let float_claims_escape = self
            .overlays
            .iter()
            .any(|o| matches!(o, Overlay::Float(f, _, _) if f.focusable))
            && matches!(
                event,
                UiEvent::KeyPressed {
                    key: Key::Named(NamedKey::Escape),
                    ..
                }
            );
        if float_claims_escape {
            self.dismiss_float(backend);
            return Some(Reaction::Redraw);
        }

        None
    }

    /// The full `event -> hit -> semantic` orchestration: menu-bar/
    /// context-menu routing, divider/scrollbar drag arming and translation,
    /// click-to-focus, focus-scoped key/paste routing, and the raw ->
    /// semantic event substitution, ending in exactly one call to `sink`
    /// (two, for a `MouseDown`/`DoubleClick` that also changes focus) with
    /// the fully-resolved event. Mirrors a binding's own `AppLogic::handle`
    /// body one-for-one; the binding itself need only check whether its own
    /// handler is registered before calling this (so that an app with no
    /// handler at all skips every bit of state this engine would otherwise
    /// touch) and supply `sink`.
    pub fn handle(
        &mut self,
        mut event: UiEvent,
        backend: &mut dyn Backend,
        sink: &mut dyn EventSink,
    ) -> quadraui::Reaction {
        use quadraui::Reaction;

        // Set when a tooltip auto-dismisses below — merged into the final
        // `Reaction` (and threaded through `continue_or_tooltip_redraw`
        // for the handful of early `return`s further down whose own
        // outcome is "nothing changed") so the redraw that takes the
        // tooltip off screen always happens, regardless of which path
        // through this function the event takes.
        let mut tooltip_dismissed = false;

        // A `MouseUp` always ends an armed divider/scrollbar drag, even
        // when the menu-bar/context-menu routing just below reports the
        // same event `Consumed` (e.g. a dropdown-item click, or the
        // outside-click that dismisses an open dropdown/popup) and returns
        // before reaching the `MouseUp` check that would otherwise sit
        // after that routing — leaving a drag armed across the dismissal
        // otherwise.
        if matches!(event, UiEvent::MouseUp { .. }) {
            *self.active_drag.borrow_mut() = None;
            *self.scrollbar_drag.borrow_mut() = None;
            *self.column_drag.borrow_mut() = None;
            *self.scroll_drag.borrow_mut() = None;
            self.chrome_pressed.borrow_mut().clear();
            // Mirrors the resets above for a `"form"` extra's own
            // scrollbar-thumb drag: `FormController::handle_event`'s `MouseUp`
            // arm clears `scroll_drag` unconditionally (it doesn't check
            // position), so trying every cached controller here releases
            // whichever one (if any) is mid-drag instead of leaving its
            // thumb rendered pressed forever (`build_scrollbar_lh`'s
            // `dragging` flag never clears otherwise).
            for id in self.form_order.borrow().iter() {
                let rect = self.form_rects.borrow().get(id).copied();
                if let Some(rect) = rect {
                    if let Some(controller) = self.form_controllers.borrow_mut().get_mut(id) {
                        let _ = controller.handle_event(&event, backend, rect);
                    }
                }
            }
        }

        // A tooltip is non-interactive chrome, not a modal-input state
        // machine — it never claims a click/key for itself, so this is a
        // side effect rather than an early return. Clearing it here (on
        // every `MouseDown`/`KeyPressed`, before anything else below gets
        // a chance to consume the event) is what "route dismissal" means
        // for a primitive with [`quadraui::Backend::draw_tooltip`]'s "no
        // hit data returned" contract: the next interaction anywhere always hides
        // it, same as a native tooltip's hover-out behaviour.
        if matches!(
            event,
            UiEvent::MouseDown { .. } | UiEvent::KeyPressed { .. }
        ) && self.hide_tooltip()
        {
            // Not returned directly — `tooltip_dismissed` is merged into
            // the final `Reaction` below, so a click that both dismisses
            // the tooltip and goes on to do something else (e.g. focus a
            // button) still redraws for the dismissal even when nothing
            // else would have.
            tooltip_dismissed = true;
        }

        // A directly-dispatched `UiEvent::FocusChanged` — e.g. a binding's
        // own Tab-cycling, which calls `AppLogic::handle` (hence this
        // engine, once the binding's own `handle` forwards to it) straight
        // through with the new focus, never going through the
        // click-to-focus bookkeeping further down — gets the same
        // "scroll a child only partly inside its `Scroll` fully into
        // view" side effect that bookkeeping already applies to a
        // click-driven focus change.
        let mut scroll_into_view_reaction = Reaction::Continue;
        if let UiEvent::FocusChanged(Some(id)) = &event {
            scroll_into_view_reaction = self.scroll_focused_child_into_view(*id, sink);
            if matches!(scroll_into_view_reaction, Reaction::Exit) {
                return scroll_into_view_reaction;
            }
        }

        // One shared pass over `Self::overlays`, topmost (highest
        // `Overlay::rank`) first — toast (claims a `MouseDown` landing on
        // its own dismiss/action region only), then dialog (claims
        // everything), then completions/rich-text-popup (each claims
        // Escape and a `MouseDown`), then float (claims Escape only when
        // focusable) — collapsing what used to be five hand-written
        // per-kind precedence blocks into [`Self::dispatch_overlays`].
        // `None` means nothing in the stack claimed `event`; the
        // menu-bar/context-menu routing below (and everything after it)
        // runs exactly as if none of this existed.
        if let Some(reaction) = self.dispatch_overlays(&event, backend, sink) {
            return reaction.merge(continue_or_tooltip_redraw(tooltip_dismissed));
        }

        // A `"menu_bar"` extra's dropdown and a context-menu popup both own
        // a full modal-input state machine that must see every event before
        // the rest of this function's own routing — matching
        // `MenuSystem::handle`/`ContextMenuController::handle`'s own doc
        // ("call this before other UI routing"). Each reports `Ignored`
        // immediately when nothing of theirs is open/relevant, so this is
        // safe to run unconditionally even for an app with neither a menu
        // bar nor an open context menu.
        let mut menu_reaction = Reaction::Continue;
        if let Some(bar_rect) = *self.menu_bar_rect.borrow() {
            let was_open = self.menu_system.borrow().is_open();
            let menu_event = self
                .menu_system
                .borrow_mut()
                .handle(&event, backend, bar_rect);
            match menu_event {
                MenuEvent::Activated(id) => {
                    // Activation already closed the dropdown in
                    // `menu_system`'s own state; force the repaint that
                    // takes it off screen regardless of what `sink`
                    // itself returns (mirrors `focus_reaction.merge`
                    // below).
                    return Reaction::Redraw
                        .merge(sink.dispatch(&UiEvent::MenuActivated(id), None));
                }
                // Navigation/highlight only — nothing for the binding's own
                // handler to react to, just a repaint so the new
                // open/selected state shows. Exception: a `MouseMoved` with
                // no dropdown open before or after this call only updated
                // which top-level label is hover-highlighted — it isn't
                // "consuming" the move in any sense a divider/scrollbar drag
                // or the handler itself would care about, so let it keep
                // flowing through the rest of this function instead of
                // swallowing it here; this call's own repaint need folds
                // into whatever `Reaction` that routing ends up producing.
                MenuEvent::StateChanged | MenuEvent::Consumed => {
                    let hover_only_on_closed_bar = !was_open
                        && !self.menu_system.borrow().is_open()
                        && matches!(event, UiEvent::MouseMoved { .. });
                    if hover_only_on_closed_bar {
                        menu_reaction = Reaction::Redraw;
                    } else {
                        return Reaction::Redraw;
                    }
                }
                MenuEvent::Ignored => {}
            }
        }
        match self.dispatch_context_menu(&event, backend) {
            ContextMenuOutcome::Event(ev) => {
                // Same reasoning as the `MenuEvent::Activated` arm above:
                // activation/dismissal already closed the popup in
                // `ctx_menu`'s own state, so force the repaint regardless
                // of what `sink` returns.
                return Reaction::Redraw.merge(sink.dispatch(&ev, None));
            }
            ContextMenuOutcome::Consumed => return Reaction::Redraw,
            ContextMenuOutcome::Ignored => {}
        }

        // A `KeyPressed`/`CharTyped`/`ClipboardPaste` landing on a `"form"`
        // extra that currently holds engine-level focus routes through
        // that `Form`'s own cached `FormController` — `FormController::handle_event`'s
        // own `has_focus` gate (see its module doc) is what makes this
        // safe to try unconditionally even when no `"form"` extra is
        // focused (or none exists this frame): `form_rects`/`form_controllers`
        // then has no entry for the focused id, or `set_has_focus(true)`
        // simply enables a gate nothing else reads.
        //
        // Plain Tab/Shift+Tab (no Ctrl/Alt/Cmd) is special-cased ahead of
        // the generic dispatch below: `Self::tab_stops` excludes this
        // form's own stop from the app-level cycle for as long as
        // `Self::form_engine_focus` names it, specifically so the key
        // reaches `Self::handle_form_tab` here instead — see that
        // method's own doc for the "move between fields, then leave the
        // form at either end" contract.
        //
        // Up/Down falls through to the generic dispatch first, same as
        // every other key: only once that reports `Handled::Ignored`
        // (the focused field itself has no use for Up/Down) does
        // `Self::maybe_move_form_focus_vertically` treat it as a
        // field-to-field move — see that method's own doc for why trying
        // the field first is what keeps a
        // `DatePicker`/`ComboBox`/`Dropdown`/`NumberInput`/`RadioGroup`'s
        // own Up/Down handling working unchanged.
        if matches!(
            event,
            UiEvent::KeyPressed { .. } | UiEvent::CharTyped(_) | UiEvent::ClipboardPaste(_)
        ) {
            if let Some(focused) = backend.focus_manager().focused().copied() {
                if let Some(rect) = self.form_rects.borrow().get(&focused).copied() {
                    let tab_direction = match &event {
                        UiEvent::KeyPressed {
                            key: Key::Named(named @ (NamedKey::Tab | NamedKey::BackTab)),
                            modifiers,
                            ..
                        } if !modifiers.ctrl && !modifiers.alt && !modifiers.cmd => {
                            Some(matches!(named, NamedKey::Tab) && !modifiers.shift)
                        }
                        _ => None,
                    };
                    if let Some(forward) = tab_direction {
                        match self.handle_form_tab(focused, rect, forward, backend, sink) {
                            Some(FormTabOutcome::LeftForm(reaction)) => {
                                return reaction
                                    .merge(continue_or_tooltip_redraw(tooltip_dismissed));
                            }
                            Some(FormTabOutcome::Moved(form_event)) => {
                                event = UiEvent::Form(focused, form_event);
                            }
                            Some(FormTabOutcome::PassThrough) | None => {}
                        }
                    } else {
                        // Scoped so the `RefMut` this borrows is dropped
                        // before the `Handled::Ignored` arm below ever
                        // calls `Self::maybe_move_form_focus_vertically`,
                        // which borrows the same `RefCell` immutably —
                        // keeping the mutable borrow alive across that
                        // call (as a plain `if let` scrutinee would) is a
                        // guaranteed `BorrowError` panic the moment the
                        // focused field itself has no use for the key
                        // (every non-text field kind, and any field-less
                        // focus state at all).
                        let handled = self.form_controllers.borrow_mut().get_mut(&focused).map(
                            |controller| {
                                controller.set_has_focus(true);
                                controller.handle_event(&event, backend, rect)
                            },
                        );
                        if let Some(handled) = handled {
                            match handled {
                                Handled::Event(FormControllerAction::FormAction(form_event)) => {
                                    if let FormEvent::FocusChanged { id } = &form_event {
                                        apply_form_focus_changed(&self.widget_state, focused, *id);
                                    }
                                    event = UiEvent::Form(focused, form_event);
                                }
                                Handled::Event(FormControllerAction::ScrollChanged) => {}
                                Handled::Ignored => {
                                    if let Some(form_event) =
                                        self.maybe_move_form_focus_vertically(focused, &event)
                                    {
                                        event = UiEvent::Form(focused, form_event);
                                    }
                                }
                                Handled::Consumed => {}
                            }
                        }
                    }
                }
            }
        }

        // A `MouseDown` lands on at most one `"form"` extra's own `rect`
        // (`FormController::handle_event`'s own `click_inner` returns `Ignored`,
        // with no mutation, for every other one — see its doc), so trying
        // every cached controller, in `form_order`'s deterministic paint
        // order, and stopping at the first non-`Ignored` result is safe
        // regardless of how many `"form"` extras are
        // painted this frame. This runs ahead of (and independently of)
        // the generic hit-test/click-to-focus pipeline below, since
        // `FormController` resolves which FIELD within the form a click
        // landed on using its own internal layout — the generic pipeline
        // only ever sees the form AS A WHOLE (one `WidgetId`, via
        // `ExtraPaint::hit_test`'s whole-area check).
        if matches!(event, UiEvent::MouseDown { .. }) {
            let claim = self
                .form_order
                .borrow()
                .iter()
                .filter_map(|id| self.form_rects.borrow().get(id).map(|rect| (*id, *rect)))
                .collect::<Vec<_>>()
                .into_iter()
                .find_map(|(id, rect)| {
                    let outcome = self
                        .form_controllers
                        .borrow_mut()
                        .get_mut(&id)
                        .map(|controller| controller.handle_event(&event, backend, rect))?;
                    (!matches!(outcome, Handled::Ignored)).then_some((id, outcome))
                });
            if let Some((id, outcome)) = claim {
                // Mirrors the generic click-to-focus block below: a click
                // a `FormController` just claimed also moves engine-level
                // focus to the form's own id, so the `KeyPressed`/
                // `CharTyped`/`ClipboardPaste` routing above can find it
                // on the very next keystroke.
                let mut focus_reaction = Reaction::Continue;
                if backend.set_focus(Some(id)) {
                    focus_reaction = sink.dispatch(&UiEvent::FocusChanged(Some(id)), None);
                    if matches!(focus_reaction, Reaction::Exit) {
                        return focus_reaction;
                    }
                }
                return match outcome {
                    Handled::Event(FormControllerAction::FormAction(form_event)) => {
                        // A click resolving to `FocusChanged` is exactly
                        // "clicking a field focuses it" — persist it the
                        // same way Tab/Shift-Tab/Up/Down do, so an
                        // uncontrolled form (the app never echoes
                        // `focused_field` back) still keeps the clicked
                        // field focused on the next render.
                        if let FormEvent::FocusChanged { id: field_id } = &form_event {
                            apply_form_focus_changed(&self.widget_state, id, *field_id);
                        }
                        focus_reaction.merge(
                            Reaction::Redraw
                                .merge(sink.dispatch(&UiEvent::Form(id, form_event), None)),
                        )
                    }
                    Handled::Event(FormControllerAction::ScrollChanged) | Handled::Consumed => {
                        focus_reaction.merge(Reaction::Redraw)
                    }
                    Handled::Ignored => unreachable!("filtered out above"),
                };
            }
        }

        // A move with the left button held continues whichever `"form"`
        // extra's own scrollbar-thumb drag (if any) the `MouseDown` claim
        // above armed — `FormController::handle_event`'s `MouseMoved` arm
        // no-ops (`Ignored`) for every controller with no `scroll_drag` of
        // its own, so trying all of them here is safe regardless of which
        // one (if any) is actually dragging. Unlike the generic
        // divider/scrollbar drags above, `FormController`'s own
        // `scroll_offset` is internal state the host never needs to
        // mirror (same contract as its text cursor), so this never swaps
        // `event` into a semantic substitute — it just needs to run so the
        // redraw after a `Consumed` result shows the new offset.
        if matches!(
            event,
            UiEvent::MouseMoved {
                buttons: quadraui::ButtonMask { left: true, .. },
                ..
            }
        ) {
            let mut consumed = false;
            for id in self.form_order.borrow().iter() {
                let rect = self.form_rects.borrow().get(id).copied();
                if let Some(rect) = rect {
                    if let Some(controller) = self.form_controllers.borrow_mut().get_mut(id) {
                        if matches!(
                            controller.handle_event(&event, backend, rect),
                            Handled::Consumed | Handled::Event(FormControllerAction::ScrollChanged)
                        ) {
                            consumed = true;
                        }
                    }
                }
            }
            if consumed {
                return Reaction::Redraw;
            }
        }

        // A wheel `Scroll` landing inside a `"form"` extra's own rect
        // scrolls it directly through the cached `FormController` — unlike
        // the tree/text-display wheel-scroll translation below, this never
        // swaps `event` into a semantic substitute (the host never needs
        // to mirror `FormController`'s internal `scroll_offset`, same as
        // the drag-continuation case just above), so a hit is consumed
        // outright here rather than falling through to the generic
        // hit-test/semantic pipeline.
        if let UiEvent::Scroll { position, .. } = &event {
            let claim = self
                .form_order
                .borrow()
                .iter()
                .filter_map(|id| self.form_rects.borrow().get(id).map(|rect| (*id, *rect)))
                .collect::<Vec<_>>()
                .into_iter()
                .find(|(_, rect)| rect.contains(*position));
            if let Some((id, rect)) = claim {
                if let Some(controller) = self.form_controllers.borrow_mut().get_mut(&id) {
                    if matches!(
                        controller.handle_event(&event, backend, rect),
                        Handled::Consumed | Handled::Event(FormControllerAction::ScrollChanged)
                    ) {
                        return Reaction::Redraw;
                    }
                }
            }
        }

        // Chrome-bar (`Toolbar`/`StatusBar`/`ActivityBar`) hover bookkeeping,
        // read back by `chrome_interaction` so `render` paints real hover
        // state. Handled directly via `chrome_hover_hit` rather than the
        // `hit`/`hit_test_painted` computed below, since `event_position`
        // never resolves a position for `MouseMoved` (enriching its own
        // `widget` field isn't this module's job) — so `hit_test_painted`
        // never runs for it either. A hover transition folds a `Redraw`
        // into `menu_reaction` (merged into the final `Reaction` below) —
        // otherwise a slot-based `insert_*` host with no render-callback
        // promotion (unlike `quadraui-py`'s `reaction_for_callback`) would
        // record the new hovered child and never actually repaint it.
        if let UiEvent::MouseMoved { position, .. } = &event {
            let chrome_hit = chrome_hover_hit(&self.hit_map.borrow(), *position);
            let mut hover = self.chrome_hover.borrow_mut();
            let before = hover.clone();
            hover.retain(|bar, _| chrome_hit.map(|(hit_bar, _)| hit_bar) == Some(*bar));
            if let Some((bar, action)) = chrome_hit {
                hover.insert(bar, action);
            }
            if *hover != before {
                menu_reaction = menu_reaction.merge(Reaction::Redraw);
            }
        }

        // Whole-widget hover bookkeeping — see `Self::hover`'s own doc.
        // Synthesizes `UiEvent::MouseEntered`/`UiEvent::MouseLeft` through
        // `sink` so a binding's own handler gets a real hit-tested hover
        // signal instead of re-deriving hit-testing itself from the raw
        // `MouseMoved` position, mirroring the chrome-bar hover bookkeeping
        // just above but at `hit_test_painted`'s full granularity (toolbar
        // button, tree row, list item, details-pane body, ...) rather than
        // `chrome_hover_hit`'s narrower chrome-bar-child scope. Each
        // synthesized event carries its own `HitDetail` the same way a
        // click would, so a row-to-row move within the same `Tree` (same
        // `WidgetId`, different detail) still reaches the binding as a
        // hover change. Skipped while a divider/scrollbar/column drag is
        // armed — the `MouseMoved` below is about to become that drag's
        // own semantic substitute (`SplitDividerDragged`/
        // `ScrollOffsetChanged`/`ColumnResized`), not a real pointer move
        // over whatever the drag happens to pass over, so synthesizing a
        // hover change here would be event-handler noise rather than
        // signal.
        let mut hover_reaction = Reaction::Continue;
        let drag_armed = self.active_drag.borrow().is_some()
            || self.scrollbar_drag.borrow().is_some()
            || self.column_drag.borrow().is_some()
            || self.scroll_drag.borrow().is_some();
        if let UiEvent::MouseMoved { position, .. } = &event {
            if !drag_armed {
                let hit_now = hit_test_painted(&self.hit_map.borrow(), *position);
                let changed = *self.hover.borrow() != hit_now;
                if changed {
                    if let Some((old_id, old_detail)) = self.hover.borrow_mut().take() {
                        hover_reaction =
                            hover_reaction.merge(sink.dispatch(
                                &UiEvent::MouseLeft { widget: old_id },
                                Some(&old_detail),
                            ));
                    }
                    if matches!(hover_reaction, Reaction::Exit) {
                        return hover_reaction;
                    }
                    if let Some((new_id, new_detail)) = &hit_now {
                        hover_reaction = hover_reaction.merge(sink.dispatch(
                            &UiEvent::MouseEntered { widget: *new_id },
                            Some(new_detail),
                        ));
                    }
                    *self.hover.borrow_mut() = hit_now;
                    if matches!(hover_reaction, Reaction::Exit) {
                        return hover_reaction;
                    }
                }
            }
        }

        // A move while a divider drag is armed becomes the drag itself —
        // checked before the hit-test below, since `MouseMoved` has no
        // `widget` field for `event_position`/`set_event_widget` to touch.
        // A move while a tree scrollbar thumb drag is armed instead becomes
        // a `ScrollOffsetChanged`, a move while a `DataTable` column
        // divider drag is armed becomes a `ColumnResized`, and a move while
        // a `Scroll` scrollbar thumb drag is armed becomes another
        // `ScrollOffsetChanged` (this one also persisted via
        // `apply_scroll_offset`, so an uncontrolled `Scroll` actually moves
        // — see `ScrollDrag`'s own doc for why it needs its own branch
        // rather than reusing the tree's); all four drags are mutually
        // exclusive, so only one of these branches ever fires for a given
        // move.
        if let UiEvent::MouseMoved { position, .. } = &event {
            if let Some(drag) = self.active_drag.borrow().as_ref() {
                if let Some(new_ratio) = drag.ratio_at(*position) {
                    event = UiEvent::SplitDividerDragged {
                        tree: drag.tree(),
                        split_index: drag.split_index(),
                        new_ratio,
                    };
                }
            } else if let Some(drag) = self.scrollbar_drag.borrow().as_ref() {
                if let Some(new_offset) = drag.drag.offset_at(position.y) {
                    let new_offset = new_offset.round() as usize;
                    event = UiEvent::ScrollOffsetChanged {
                        widget: drag.widget,
                        new_offset,
                    };
                }
            } else if let Some(drag) = self.column_drag.borrow().as_ref() {
                event = UiEvent::DataTable(
                    drag.widget,
                    DataTableEvent::ColumnResized {
                        col: drag.col,
                        width: drag.width_at(*position),
                    },
                );
            } else if let Some(drag) = self.scroll_drag.borrow().as_ref() {
                let pos = match drag.axis {
                    ScrollAxis::Vertical => position.y,
                    ScrollAxis::Horizontal => position.x,
                };
                if let Some(new_offset) = drag.drag.offset_at(pos) {
                    apply_scroll_offset(&self.widget_state, drag.widget, new_offset);
                    event = UiEvent::ScrollOffsetChanged {
                        widget: drag.widget,
                        new_offset: new_offset.round() as usize,
                    };
                }
            }
        }

        // A plain `KeyPressed` (Tab/Shift+Tab already claimed upstream by
        // the binding's own `preprocess_event` once `tab_stops` is
        // non-empty) is routed to whichever leaf currently holds focus,
        // wrapped into that leaf's own `KeyPressed` variant so the binding's
        // own handler sees a focus-scoped event instead of a backend-global
        // one.
        //
        // For a focused `Tree`/`List`/`DataTable`, a recognized navigation
        // key (Up/Down/vim `j`/`k`/Home/End/PageUp/PageDown) is tried
        // against [`apply_tree_nav_key`]/[`apply_list_nav_key`]/
        // [`apply_data_table_nav_key`] first: it moves and scrolls-into-view
        // the id's cursor in [`Self::widget_state`] (controlled or not — see
        // those functions' own doc), using the hit's own row/viewport
        // geometry rather than a stale descriptor. `tree_rows_and_viewport`
        // returns `None` while the focused tree's selected row is mid-rename
        // (`editing.is_some()`), so a rename's own text-editing keys
        // (`Home`/`End` included) are never also interpreted as cursor
        // navigation. This never changes what reaches `sink` (still the same
        // `focus_scoped_key_event` result a binding always got), only
        // forces the redraw that paints the moved cursor, even when the
        // binding's own `sink.dispatch` leaves the key unhandled.
        if let UiEvent::KeyPressed { key, modifiers, .. } = &event {
            if let Some(focused) = backend.focus_manager().focused().copied() {
                let nav_applied = if let Some((row_paths, viewport_rows)) =
                    tree_rows_and_viewport(&self.hit_map.borrow(), focused)
                {
                    apply_tree_nav_key(&self.widget_state, focused, &row_paths, viewport_rows, key)
                } else if let Some((total_items, viewport_rows)) =
                    list_rows_and_viewport(&self.hit_map.borrow(), focused)
                {
                    apply_list_nav_key(&self.widget_state, focused, total_items, viewport_rows, key)
                } else if let Some((total_rows, viewport_rows)) =
                    data_table_rows_and_viewport(&self.hit_map.borrow(), focused)
                {
                    apply_data_table_nav_key(
                        &self.widget_state,
                        focused,
                        total_rows,
                        viewport_rows,
                        key,
                    )
                } else if matches!(
                    key,
                    Key::Named(NamedKey::PageUp)
                        | Key::Named(NamedKey::PageDown)
                        | Key::Named(NamedKey::Home)
                        | Key::Named(NamedKey::End)
                ) && !focused_leaf_claims_page_keys(&self.hit_map.borrow(), focused)
                {
                    // The focused leaf has none of `Tree`/`List`/`DataTable`'s
                    // own page/home/end cursor handling (those already
                    // claimed the key above, so this never double-handles
                    // one of them), and isn't a `TextInput`/`TextDisplay`
                    // that claims these keys for itself either (guarded by
                    // `focused_leaf_claims_page_keys` above, so a focused
                    // `TextInput`'s `Home`/`End` caret move and a focused
                    // `TextDisplay`'s own key handling still reach
                    // `focus_scoped_key_event` below, never this routing) —
                    // if it's a direct child of a `Scroll`, route the key to
                    // that `Scroll`'s own `ScrollView::handle_key` instead,
                    // mirroring the wheel handling below but for the
                    // keyboard, and returning straight away with a
                    // `ScrollOffsetChanged` rather than falling into the
                    // generic `focus_scoped_key_event` substitution below.
                    // `false` (not applied) when there's no enclosing
                    // `Scroll`, or its content already fits/is already at
                    // that bound — the key then falls through unchanged to
                    // `focus_scoped_key_event`, same as any other key with
                    // no navigation meaning here.
                    let scrolled = enclosing_scroll_child(&self.hit_map.borrow(), focused)
                        .and_then(|(scroll_id, ..)| {
                            let mut view = scroll_view_at(&self.hit_map.borrow(), scroll_id)?;
                            view.focused = true;
                            let step = match view.axis {
                                ScrollAxis::Vertical => backend.line_height(),
                                ScrollAxis::Horizontal => backend.char_width(),
                            };
                            let ScrollViewEvent::Scrolled { offset: new_offset } =
                                view.handle_key(key, modifiers, step)?;
                            Some((scroll_id, new_offset))
                        });
                    if let Some((scroll_id, new_offset)) = scrolled {
                        apply_scroll_offset(&self.widget_state, scroll_id, new_offset);
                        event = UiEvent::ScrollOffsetChanged {
                            widget: scroll_id,
                            new_offset: new_offset.round() as usize,
                        };
                        return Reaction::Redraw.merge(sink.dispatch(&event, None));
                    }
                    false
                } else {
                    false
                };
                event = focus_scoped_key_event(
                    focused,
                    key,
                    *modifiers,
                    &self.hit_map.borrow(),
                    &self.text_editors,
                );
                if nav_applied {
                    return Reaction::Redraw.merge(sink.dispatch(&event, None));
                }
            }
        }

        // IME/compose and clipboard-paste text has no `KeyPressed` of its
        // own to route through `focus_scoped_key_event` above — resolve it
        // the same way, directly against whichever `TextInput` extra
        // currently holds focus.
        if let UiEvent::CharTyped(ch) = &event {
            if let Some(focused) = backend.focus_manager().focused().copied() {
                if let Some(form_event) = text_input_insert_event(
                    focused,
                    &self.hit_map.borrow(),
                    EditOp::InsertChar(*ch),
                    &self.text_editors,
                ) {
                    event = form_event;
                }
            }
        }
        if let UiEvent::ClipboardPaste(text) = &event {
            if let Some(focused) = backend.focus_manager().focused().copied() {
                if let Some(form_event) = text_input_insert_event(
                    focused,
                    &self.hit_map.borrow(),
                    EditOp::InsertText(text.clone()),
                    &self.text_editors,
                ) {
                    event = form_event;
                }
            }
        }

        let hit = event_position(&event)
            .and_then(|position| hit_test_painted(&self.hit_map.borrow(), position));
        if let Some((id, _)) = hit {
            set_event_widget(&mut event, id);
        }

        // A `MouseDown` landing on a chrome-bar child presses it, mirroring
        // `Self::chrome_hover`'s bookkeeping above — cleared unconditionally
        // on the next `MouseUp` (top of this function), regardless of
        // where that `MouseUp` lands.
        if let (UiEvent::MouseDown { .. }, Some((action, HitDetail::ExtraChrome { bar }))) =
            (&event, &hit)
        {
            self.chrome_pressed.borrow_mut().insert(*bar, *action);
        }

        // A wheel `Scroll` landing anywhere over a tree (its rows, its
        // chevrons, or its scrollbar) replaces the raw event outright with
        // the `ScrollOffsetChanged` it resolves to — never dispatched
        // alongside it — mirroring the `MouseDown`/`DoubleClick` semantic
        // replacement below, including suppressing the `"hit"` attachment
        // via `scroll_translated`, below. A tree with nothing to scroll
        // (all rows fit the viewport) has no scrollbar, so
        // `tree_scroll_state` returns `None` and the raw `Scroll` passes
        // through untouched. A `TextDisplay` scrolls the same way, but
        // into its own `TextDisplayEvent::Scrolled` — the primitive
        // already has a typed scroll event, unlike `Tree`, so there is no
        // need for the generic `ScrollOffsetChanged` substitute. A
        // `LayoutNode::Scroll` container (its body or its own scrollbar)
        // scrolls through `quadraui::ScrollView::handle_wheel` instead —
        // this crate's own math never re-derives what that primitive
        // already computes — persisting the result via
        // `apply_scroll_offset` the same way the drag/key handling below
        // does, so an uncontrolled `Scroll` actually moves on the next
        // render.
        let mut scroll_translated = false;
        if let UiEvent::Scroll { delta, .. } = &event {
            if let Some((id, detail)) = &hit {
                let over_tree = matches!(
                    detail,
                    HitDetail::TreeRow { .. }
                        | HitDetail::TreeChevron { .. }
                        | HitDetail::TreeScrollbarThumb
                        | HitDetail::TreeScrollbarTrackBefore
                        | HitDetail::TreeScrollbarTrackAfter
                );
                let over_text_display = matches!(
                    detail,
                    HitDetail::TextDisplayLine { .. }
                        | HitDetail::TextDisplayScrollbarThumb
                        | HitDetail::TextDisplayScrollbarTrackBefore
                        | HitDetail::TextDisplayScrollbarTrackAfter
                );
                // A `"markdown"` extra leaf is a `TextDisplay` under the
                // hood, but it paints as `PaintedHit::Extra` (so its link
                // chrome hits can coexist with it), not
                // `PaintedHit::TextDisplay` — `ExtraBody`/`ExtraChrome` is
                // what `hit_test_painted` reports for it instead of
                // `TextDisplayLine`, so it needs this separate lookup
                // rather than folding into `over_text_display` above.
                // `ExtraChrome`'s own `id` is the clicked link's child id,
                // not the leaf's — `bar` is, matching every other
                // `ExtraChrome` resolution in this file (see
                // `semantic_event_for_hit`'s own `bar` use).
                let over_markdown = match detail {
                    HitDetail::ExtraBody => Some(*id),
                    HitDetail::ExtraChrome { bar } => Some(*bar),
                    _ => None,
                }
                .and_then(|markdown_id| {
                    markdown_at(&self.hit_map.borrow(), markdown_id)
                        .map(|(area, markdown)| (markdown_id, area, markdown))
                });
                // Either landed on the `Scroll` itself (its empty body or
                // its own scrollbar) or on a direct child with no wheel
                // behavior of its own (a plain `Button`, say) that happens
                // to be inside one — `over_tree`/`over_text_display` above
                // already claimed every kind that scrolls itself, so a
                // child reaching here never double-handles a wheel notch
                // its own primitive would otherwise have consumed.
                let scroll_target = if matches!(
                    detail,
                    HitDetail::ScrollBody
                        | HitDetail::ScrollScrollbarThumb
                        | HitDetail::ScrollScrollbarTrackBefore
                        | HitDetail::ScrollScrollbarTrackAfter
                ) {
                    Some(*id)
                } else {
                    enclosing_scroll_child(&self.hit_map.borrow(), *id)
                        .map(|(scroll_id, ..)| scroll_id)
                };
                if over_tree {
                    if let Some((_, total_rows, viewport_rows, scroll_offset)) =
                        tree_scroll_state(&self.hit_map.borrow(), *id)
                    {
                        let max_offset = total_rows.saturating_sub(viewport_rows);
                        let rows = wheel_row_delta(delta.y, 1.0);
                        let new_offset = scroll_by_clamped(scroll_offset, rows, max_offset);
                        event = UiEvent::ScrollOffsetChanged {
                            widget: *id,
                            new_offset,
                        };
                        scroll_translated = true;
                    }
                } else if over_text_display {
                    if let Some((total_lines, viewport_lines, scroll_offset)) =
                        text_display_scroll_state(&self.hit_map.borrow(), *id)
                    {
                        let max_offset = total_lines.saturating_sub(viewport_lines);
                        let rows = wheel_row_delta(delta.y, 1.0);
                        let new_offset = scroll_by_clamped(scroll_offset, rows, max_offset);
                        event =
                            UiEvent::TextDisplay(*id, TextDisplayEvent::Scrolled { new_offset });
                        scroll_translated = true;
                    }
                } else if let Some((markdown_id, area, markdown)) = over_markdown {
                    let (total_lines, viewport_lines) = markdown.viewport_lines(backend, area);
                    let max_offset = total_lines.saturating_sub(viewport_lines);
                    let rows = wheel_row_delta(delta.y, 1.0);
                    let new_offset = scroll_by_clamped(markdown.scroll_offset, rows, max_offset);
                    event = UiEvent::TextDisplay(
                        markdown_id,
                        TextDisplayEvent::Scrolled { new_offset },
                    );
                    scroll_translated = true;
                } else if let Some(scroll_id) = scroll_target {
                    if let Some(view) = scroll_view_at(&self.hit_map.borrow(), scroll_id) {
                        let step = match view.axis {
                            ScrollAxis::Vertical => backend.line_height(),
                            ScrollAxis::Horizontal => backend.char_width(),
                        };
                        if let Some(ScrollViewEvent::Scrolled { offset: new_offset }) =
                            view.handle_wheel(*delta, step)
                        {
                            apply_scroll_offset(&self.widget_state, scroll_id, new_offset);
                            event = UiEvent::ScrollOffsetChanged {
                                widget: scroll_id,
                                new_offset: new_offset.round() as usize,
                            };
                            scroll_translated = true;
                        }
                    }
                }
            }
        }

        // A `MouseDown` (or a bare `DoubleClick` pushed without the
        // `MouseDown`/`MouseUp` pair that precedes one on a real backend —
        // a host synthesizing clicks, e.g. a test, does this) landing on a
        // focusable painted leaf moves keyboard focus there first — matching
        // every native backend's click-to-focus convention — and, if that
        // actually changed the focused widget, dispatches a `FocusChanged`
        // event through `sink` ahead of the click event itself. Its
        // `Reaction` is combined with the main dispatch's below (via
        // `Reaction::merge`) rather than returned on its own, so a
        // non-`Continue` result from the `FocusChanged` dispatch (e.g. a
        // `Redraw`) never suppresses the click event that follows it.
        let mut focus_reaction = Reaction::Continue;
        if let Some((id, _)) = &hit {
            if matches!(
                event,
                UiEvent::MouseDown { .. } | UiEvent::DoubleClick { .. }
            ) {
                let is_focusable = self.tab_stops().iter().any(|(tid, _)| tid == id);
                if is_focusable && backend.set_focus(Some(*id)) {
                    focus_reaction = sink.dispatch(&UiEvent::FocusChanged(Some(*id)), None);
                    if matches!(focus_reaction, Reaction::Exit) {
                        return focus_reaction;
                    }
                    // The newly-focused child may be a direct child of a
                    // `Scroll`, painted only partly inside its viewport
                    // (straddling an edge) — scroll it fully into view, the
                    // same way a focused `Tree`/`List`/`DataTable` row
                    // scrolls itself into view on cursor navigation.
                    focus_reaction =
                        focus_reaction.merge(self.scroll_focused_child_into_view(*id, sink));
                    if matches!(focus_reaction, Reaction::Exit) {
                        return focus_reaction;
                    }
                }
            }
        }

        // A `MouseDown`/`DoubleClick` either arms a divider drag or, for a
        // tab/tree-row/tree-chevron/list-item/data-table-row/data-table-header
        // hit, is replaced outright by the semantic event it stands for — in
        // which case the raw `MouseDown`/`DoubleClick` (and its `"hit"` dict)
        // never reaches `sink` at all.
        let mut attach_hit = hit.is_some() && !scroll_translated;
        // Set by the `HitDetail::ButtonBody` arm below, alongside (never
        // instead of) the raw `MouseDown`/`DoubleClick` dispatch — merged
        // into the final `Reaction` the same way `focus_reaction` is.
        let mut button_clicked_reaction = Reaction::Continue;
        if let Some((id, detail)) = &hit {
            let modifiers_and_double_click = match &event {
                UiEvent::MouseDown { modifiers, .. } => Some((*modifiers, false)),
                UiEvent::DoubleClick { .. } => Some((Modifiers::default(), true)),
                _ => None,
            };
            if let Some((modifiers, double_click)) = modifiers_and_double_click {
                // A non-double click landing on ANY extra's body — not just
                // `TextInput`/`NumberInput` — gets `HitDetail::ExtraBody`
                // (`hit_test_painted`'s own `Extra` arm doesn't distinguish
                // by kind), so this is the first arm of the same `if`/
                // `else if` chain the other `detail` variants use below,
                // with its own `semantic_event_for_hit` fallback folded in
                // rather than left to the chain's final arm — a `Switch`
                // body click (which `text_input_click_event`/
                // `number_input_click_event` both no-op on, since neither
                // finds the kind it looks for) still resolves through
                // `semantic_event_for_hit`'s own `ExtraBody` arm this way.
                // Folding every `ExtraBody` outcome into one arm up front
                // makes it structurally impossible for two of them to fire
                // for the same click as a future kind is wired in here.
                // Must run before `semantic_event_for_hit` is reached,
                // since `TextInput`'s caret placement is a side effect on
                // `text_editors` only (a `SetCursor` never changes the
                // buffer, so there is no event to swap in; see
                // `text_input_click_event`'s own doc) and `NumberInput`'s
                // stepper click DOES swap `event` outright.
                if !double_click && matches!(detail, HitDetail::ExtraBody) {
                    if let Some(position) = event_position(&event) {
                        text_input_click_event(
                            &self.hit_map.borrow(),
                            *id,
                            position,
                            quadraui::TextInputMeasure::from_metrics(&backend.measure()),
                            &self.text_editors,
                        );
                        if let Some(number_input_event) =
                            number_input_click_event(&self.hit_map.borrow(), *id, position)
                        {
                            event = number_input_event;
                            attach_hit = false;
                        } else if let Some(semantic) = semantic_event_for_hit(
                            *id,
                            detail,
                            modifiers,
                            double_click,
                            &self.hit_map.borrow(),
                            position,
                        ) {
                            event = semantic;
                            attach_hit = false;
                        }
                    }
                } else if !double_click && matches!(detail, HitDetail::SplitDivider) {
                    if let UiEvent::MouseDown { position, .. } = &event {
                        if let Some(layout) =
                            find_split_layout(&self.hit_map.borrow(), *position, *id)
                        {
                            *self.active_drag.borrow_mut() =
                                Some(ActiveDrag::Split { tree: *id, layout });
                        }
                    }
                } else if !double_click && matches!(detail, HitDetail::SplitTreeDivider { .. }) {
                    // Mirrors the `SplitDivider` arm above: arms a drag
                    // (consumed by the `MouseMoved` branch above on later
                    // moves) for the N-way `SplitTree` case instead of the
                    // two-pane `Split` one, keyed by the dragged divider's
                    // own `split_index` rather than always `0`.
                    if let HitDetail::SplitTreeDivider { split_index } = detail {
                        if let Some(layout) = find_split_tree_layout(&self.hit_map.borrow(), *id) {
                            *self.active_drag.borrow_mut() = Some(ActiveDrag::SplitTree {
                                tree: *id,
                                split_index: *split_index,
                                layout,
                            });
                        }
                    }
                } else if !double_click
                    && matches!(detail, HitDetail::DataTableHeaderDivider { .. })
                {
                    // Mirrors the `SplitDivider` arm above: arms the drag
                    // (consumed by the `MouseMoved` branch above on later
                    // moves) but leaves the raw `MouseDown` (and its
                    // `"hit"` dict) to reach `sink` unchanged below, same as
                    // a split-divider press.
                    if let HitDetail::DataTableHeaderDivider { col } = detail {
                        if let Some((area, layout)) =
                            find_data_table_layout(&self.hit_map.borrow(), *id)
                        {
                            *self.column_drag.borrow_mut() = Some(DataTableColumnDrag {
                                widget: *id,
                                col: *col,
                                area,
                                layout,
                            });
                        }
                    }
                } else if !double_click && matches!(detail, HitDetail::TreeScrollbarThumb) {
                    // A press on the thumb itself arms a drag (consumed by
                    // the `MouseMoved` branch above on later moves) rather
                    // than scrolling immediately — matching
                    // `compose::TreeController::click_scrollbar`'s
                    // `PressZone::Thumb` arm. The raw `MouseDown` (and its
                    // `"hit"` dict) stops here either way, so this never
                    // falls through to row selection.
                    if let UiEvent::MouseDown { position, .. } = &event {
                        if let Some((scrollbar, total_rows, viewport_rows, scroll_offset)) =
                            tree_scroll_state(&self.hit_map.borrow(), *id)
                        {
                            let max_offset = total_rows.saturating_sub(viewport_rows);
                            if max_offset > 0 {
                                *self.scrollbar_drag.borrow_mut() = Some(ScrollbarDrag {
                                    widget: *id,
                                    drag: ThumbDrag::begin(
                                        position.y,
                                        scroll_offset as f32,
                                        scrollbar.travel(),
                                        max_offset as f32,
                                    ),
                                });
                            }
                        }
                    }
                    attach_hit = false;
                } else if !double_click
                    && matches!(
                        detail,
                        HitDetail::TreeScrollbarTrackBefore | HitDetail::TreeScrollbarTrackAfter
                    )
                {
                    // A press on the track pages by one viewport, matching
                    // `click_scrollbar`'s `PressZone::Before`/`After` arms
                    // (`page_scroll`). Never falls through to row selection.
                    if let Some((_, total_rows, viewport_rows, scroll_offset)) =
                        tree_scroll_state(&self.hit_map.borrow(), *id)
                    {
                        let max_offset = total_rows.saturating_sub(viewport_rows);
                        let page = if matches!(detail, HitDetail::TreeScrollbarTrackBefore) {
                            -(viewport_rows as isize)
                        } else {
                            viewport_rows as isize
                        };
                        let new_offset = scroll_by_clamped(scroll_offset, page, max_offset);
                        event = UiEvent::ScrollOffsetChanged {
                            widget: *id,
                            new_offset,
                        };
                    }
                    attach_hit = false;
                } else if !double_click && matches!(detail, HitDetail::ScrollScrollbarThumb) {
                    // Mirrors the `TreeScrollbarThumb` arm above: a press on
                    // the thumb itself arms a drag (consumed by the
                    // `MouseMoved` branch above on later moves) rather than
                    // scrolling immediately.
                    if let UiEvent::MouseDown { position, .. } = &event {
                        if let Some(scrollbar) = scroll_scrollbar(&self.hit_map.borrow(), *id) {
                            if let Some(view) = scroll_view_at(&self.hit_map.borrow(), *id) {
                                let max_offset = view.max_offset();
                                if max_offset > 0.0 {
                                    let pos = match scrollbar.axis {
                                        ScrollAxis::Vertical => position.y,
                                        ScrollAxis::Horizontal => position.x,
                                    };
                                    *self.scroll_drag.borrow_mut() = Some(ScrollDrag {
                                        widget: *id,
                                        axis: scrollbar.axis,
                                        drag: ThumbDrag::begin(
                                            pos,
                                            view.scroll_offset,
                                            scrollbar.travel(),
                                            max_offset,
                                        ),
                                    });
                                }
                            }
                        }
                    }
                    attach_hit = false;
                } else if !double_click
                    && matches!(
                        detail,
                        HitDetail::ScrollScrollbarTrackBefore
                            | HitDetail::ScrollScrollbarTrackAfter
                    )
                {
                    // Mirrors the `TreeScrollbarTrackBefore`/`TreeScrollbarTrackAfter`
                    // arm above: a press on the bare track pages by one
                    // viewport, through `ScrollView::scroll_by` rather than
                    // re-deriving the page delta.
                    if let Some(view) = scroll_view_at(&self.hit_map.borrow(), *id) {
                        let page = if matches!(detail, HitDetail::ScrollScrollbarTrackBefore) {
                            -view.visible_len()
                        } else {
                            view.visible_len()
                        };
                        if let Some(ScrollViewEvent::Scrolled { offset: new_offset }) =
                            view.scroll_by(page)
                        {
                            apply_scroll_offset(&self.widget_state, *id, new_offset);
                            event = UiEvent::ScrollOffsetChanged {
                                widget: *id,
                                new_offset: new_offset.round() as usize,
                            };
                        }
                    }
                    attach_hit = false;
                } else if matches!(detail, HitDetail::ButtonBody) {
                    // A layout-tree `Button` leaf's click is additive, not
                    // a replacement, unlike every other `detail` arm above
                    // and below: `event`/`attach_hit` are left untouched
                    // here, so the raw `MouseDown`/`DoubleClick` (with its
                    // `"hit"` dict still attached) reaches `sink` below
                    // exactly as it always did, for compatibility with a
                    // binding's own handler that already synthesizes the
                    // click itself from that raw event. This dispatch is
                    // the semantic event on top of it.
                    button_clicked_reaction =
                        sink.dispatch(&button_clicked_event(*id), Some(&HitDetail::ButtonBody));
                    if matches!(button_clicked_reaction, Reaction::Exit) {
                        return button_clicked_reaction;
                    }
                } else if let Some(semantic) = semantic_event_for_hit(
                    *id,
                    detail,
                    modifiers,
                    double_click,
                    &self.hit_map.borrow(),
                    event_position(&event).unwrap_or_default(),
                ) {
                    event = semantic;
                    attach_hit = false;
                }
            }
        }

        // Persists the handful of events `Self::widget_state` tracks
        // uncontrolled-by-default, regardless of what `sink` goes on to do
        // with them: a divider drag's resolved ratio
        // ([`crate::host::resolve_split_ratio`]'s own counterpart), a
        // clicked tab becoming active or a closed tab shifting whichever
        // index was active ([`crate::host::resolve_tab_bar_active`]'s), or
        // a toggled tree row's expand state
        // ([`crate::host::resolve_tree_expansion`]'s). Never changes
        // `event` itself — a controlled app's own handling of the same
        // event is unaffected. `persisted_state` mirrors the Tree-cursor
        // hook above (`nav_applied`'s own `Reaction::Redraw.merge`): a
        // binding with no render callback of its own (so
        // `host::callback_reaction_for_render`'s render-triggered redraw
        // never kicks in) still needs the frame that paints the persisted
        // change forced here, even when `sink.dispatch` below leaves the
        // event itself unhandled.
        let mut persisted_state = Reaction::Continue;
        match &event {
            UiEvent::SplitDividerDragged {
                tree,
                split_index,
                new_ratio,
            } => {
                apply_split_divider_drag(&self.widget_state, *tree, *split_index, *new_ratio);
                persisted_state = Reaction::Redraw;
            }
            UiEvent::TabBar(id, TabBarEvent::TabActivated { index }) => {
                apply_tab_activated(&self.widget_state, *id, *index);
                persisted_state = Reaction::Redraw;
            }
            UiEvent::TabBar(id, TabBarEvent::TabClosed { index }) => {
                apply_tab_closed(&self.widget_state, *id, *index);
                persisted_state = Reaction::Redraw;
            }
            UiEvent::Tree(id, TreeEvent::RowToggleExpand { path }) => {
                apply_tree_toggle_expand(&self.widget_state, *id, path);
                persisted_state = Reaction::Redraw;
            }
            // Engine-owned focus move inside an uncontrolled form —
            // Tab/Shift-Tab (`Self::handle_form_tab`'s `Moved` outcome) or
            // Up/Down (`Self::maybe_move_form_focus_vertically`) both
            // rewrite `event` to this same shape. Unlike the click path a
            // few hundred lines up (which already merges its own
            // `Reaction::Redraw` before returning), neither of those two
            // keyboard paths returns early, so without this the focus
            // ring would only repaint once something else forces a frame.
            UiEvent::Form(_, FormEvent::FocusChanged { .. }) => {
                persisted_state = Reaction::Redraw;
            }
            _ => {}
        }

        let hit_detail = if attach_hit {
            hit.as_ref().map(|(_, detail)| detail)
        } else {
            None
        };
        let event_reaction = sink.dispatch(&event, hit_detail);
        let tooltip_reaction = if tooltip_dismissed {
            Reaction::Redraw
        } else {
            Reaction::Continue
        };
        tooltip_reaction
            .merge(scroll_into_view_reaction)
            .merge(menu_reaction)
            .merge(hover_reaction)
            .merge(focus_reaction)
            .merge(button_clicked_reaction)
            .merge(event_reaction)
            .merge(persisted_state)
    }
}

/// Builds a [`MessageDialogOptions`] from a binding's own decoded
/// `title`/`body`/`buttons`/`severity` arguments, validating `buttons` the
/// way every binding's own `show_message_dialog`-style surface needs to:
/// non-empty (a dialog with no buttons can never be resolved by Enter or
/// Escape) and no button setting `tint` (accepted for shape compatibility
/// with the in-canvas `Dialog` primitive's own `buttons` field, but not yet
/// honored by [`MessageDialogController`]). `spec`, unlike `tint`, carries
/// straight through to the built [`MessageDialogButton`] — honored by
/// [`MessageDialogController`] on TUI, silently ignored by the native
/// GTK/macOS/Win32 alert facilities, the same fence
/// [`quadraui::ButtonSpec`]'s own doc draws elsewhere. `Err` carries the
/// message a binding's own "invalid input" error wraps.
pub fn message_dialog_options_from_buttons(
    title: String,
    body: String,
    buttons: Vec<quadraui::DialogButton>,
    severity: Option<quadraui::DialogSeverity>,
) -> Result<MessageDialogOptions, String> {
    if buttons.is_empty() {
        return Err(
            "buttons must not be empty — a dialog with no buttons can never be resolved by \
             Enter or Escape"
                .to_string(),
        );
    }
    if let Some(tinted) = buttons.iter().find(|b| b.tint.is_some()) {
        return Err(format!(
            "button {:?} sets tint, which this surface does not honor yet — omit tint or \
             leave it unset",
            tinted.id.as_str()
        ));
    }
    Ok(MessageDialogOptions {
        title,
        body,
        buttons: buttons
            .into_iter()
            .map(|b| MessageDialogButton {
                id: b.id,
                label: b.label,
                is_default: b.is_default,
                is_cancel: b.is_cancel,
                spec: b.spec,
            })
            .collect(),
        severity,
    })
}

#[cfg(test)]
#[allow(deprecated)] // `Panel`/`PanelAction` are `#[deprecated]`; `PanelLogic` and its tests cover `LayoutNode::Panel`'s own plumbing directly.
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use std::rc::Rc;

    use quadraui::tui::testing::TuiDriver;
    use quadraui::{
        AppLogic, Backend, Button, DataTable, Key, ListView, Modifiers, MouseButton, NamedKey,
        Point, Reaction, Rect, ScrollAxis, ScrollDelta, Split, StyledSpan, TabBar, TabBarEvent,
        TabBarHit, TextDisplay, TextDisplayLine, TextInput, ToolbarEvent, TreeEvent, TreeView,
        UiEvent, WidgetId,
    };

    use super::{viewport_rect, EventSink, InteractionEngine};
    use crate::drag::find_split_tree_layout;
    use crate::hit::{HitDetail, PaintedHit};
    use crate::host::{
        resolve_data_table_cursor, resolve_layout_node, resolve_layout_tabs_active,
        resolve_list_cursor, resolve_split_ratio, resolve_tab_bar_active, resolve_tree_cursor,
        resolve_tree_expansion,
    };
    use crate::layout::{
        LayoutContainer, LayoutLeaf, LayoutNode, LayoutPanel, LayoutScroll, LayoutSplit,
        LayoutSplitTree, LayoutTabEntry, LayoutTabs, LeafDescriptor, Spacing,
    };
    use crate::render::{
        draw_bands, list_viewport_rows, paint_layout, tree_viewport_rows, RenderRefs,
    };

    /// Records every event [`InteractionEngine::handle`] dispatches, in
    /// order, so a test can assert on the orchestration's decisions (e.g.
    /// click-to-focus sending a `FocusChanged` ahead of the click itself)
    /// without any host-language callback in scope.
    #[derive(Default)]
    struct RecordingSink {
        dispatched: Vec<(UiEvent, Option<HitDetail>)>,
    }

    impl EventSink for RecordingSink {
        fn dispatch(&mut self, event: &UiEvent, hit_detail: Option<&HitDetail>) -> Reaction {
            self.dispatched.push((event.clone(), hit_detail.cloned()));
            Reaction::Continue
        }
    }

    /// A minimal [`AppLogic`] whose `render`/`handle` go through
    /// [`InteractionEngine`] exactly the way a real binding's own thin
    /// `AppLogic` leaf would, with no host-language FFI type anywhere in
    /// this file.
    struct MinimalLogic {
        button: Button,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for MinimalLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let extra = HashMap::new();
            draw_bands(
                backend,
                RenderRefs {
                    button: Some(&self.button),
                    text_display: None,
                    list: None,
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
                &|id| self.engine.chrome_interaction(id),
            );
            self.engine.render_overlays(backend);
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Mirrors [`MinimalLogic`], but paints one `"form"` extra through
    /// [`InteractionEngine::sync_forms`] instead of a plain `Button`, so a
    /// test can drive `"form"`-specific routing (scroll, drag, click)
    /// without any host-language FFI type in scope.
    struct FormLogic {
        form: quadraui::Form,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for FormLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let mut extra: HashMap<String, std::rc::Rc<dyn crate::extra_paint::ExtraPaint>> =
                HashMap::new();
            extra.insert(
                "form".to_string(),
                std::rc::Rc::new(self.form.clone())
                    as std::rc::Rc<dyn crate::extra_paint::ExtraPaint>,
            );
            draw_bands(
                backend,
                RenderRefs {
                    button: None,
                    text_display: None,
                    list: None,
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
                &|id| self.engine.chrome_interaction(id),
            );
            self.engine.sync_forms(backend, &hits);
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Mirrors [`FormLogic`], but also paints a sibling `Button` in the
    /// same main tree — needed for the "Tab leaves the form once a cycle
    /// at either end has nowhere left to go inside it" tests, which have
    /// to observe engine focus landing on something *other than* the
    /// form once Tab/Shift-Tab runs out of fields to move between.
    struct FormAndButtonLogic {
        form: quadraui::Form,
        button: Button,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for FormAndButtonLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let mut extra: HashMap<String, std::rc::Rc<dyn crate::extra_paint::ExtraPaint>> =
                HashMap::new();
            extra.insert(
                "form".to_string(),
                std::rc::Rc::new(self.form.clone())
                    as std::rc::Rc<dyn crate::extra_paint::ExtraPaint>,
            );
            draw_bands(
                backend,
                RenderRefs {
                    button: Some(&self.button),
                    text_display: None,
                    list: None,
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
                &|id| self.engine.chrome_interaction(id),
            );
            self.engine.sync_forms(backend, &hits);
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// A main-tree `"form"` extra sandwiched between two sibling `Button`
    /// stops — `before` and `after` — rather than `FormAndButtonLogic`'s
    /// single sibling. With only one sibling, "the generic app-level Tab
    /// cycle lands focus on `stops[0]`" and "`InteractionEngine::handle_form_tab`
    /// actually ran and moved focus to the next stop after the form" are
    /// indistinguishable from the outside, since both land on the same
    /// single button. Two distinct siblings tell them apart: the generic
    /// cycle run after an incorrect `sync_tab_order` always lands on
    /// `before` (`stops[0]`) regardless of which direction Tab came from,
    /// while the correct `handle_form_tab` routing leaves for whichever
    /// sibling actually sits next to the form in paint order.
    struct FormBetweenTwoButtonsLogic {
        form: quadraui::Form,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for FormBetweenTwoButtonsLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let content = LayoutNode::Column(LayoutContainer::new(
                vec![
                    LayoutNode::Leaf(LayoutLeaf {
                        id: "before".to_string(),
                        descriptor: LeafDescriptor::Button(Button::new(
                            WidgetId::new("before"),
                            "Before",
                        )),
                        scroll_px: None,
                    }),
                    LayoutNode::Leaf(LayoutLeaf {
                        id: "settings".to_string(),
                        descriptor: LeafDescriptor::Extra(Rc::new(self.form.clone())),
                        scroll_px: None,
                    }),
                    LayoutNode::Leaf(LayoutLeaf {
                        id: "after".to_string(),
                        descriptor: LeafDescriptor::Button(Button::new(
                            WidgetId::new("after"),
                            "After",
                        )),
                        scroll_px: None,
                    }),
                ],
                Spacing::default(),
            ));
            let mut hits = Vec::new();
            paint_layout(backend, area, &content, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
            self.engine.sync_forms(backend, &hits);
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    fn text_input_field(id: &str) -> quadraui::FormField {
        quadraui::FormField {
            id: WidgetId::new(id),
            label: quadraui::StyledText::plain(id),
            kind: quadraui::FieldKind::TextInput {
                value: String::new(),
                placeholder: String::new(),
                cursor: Some(0),
                selection_anchor: None,
            },
            hint: quadraui::StyledText::default(),
            disabled: false,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        }
    }

    fn toggle_field(id: &str) -> quadraui::FormField {
        quadraui::FormField {
            id: WidgetId::new(id),
            label: quadraui::StyledText::plain(id),
            kind: quadraui::FieldKind::Toggle { value: false },
            hint: quadraui::StyledText::default(),
            disabled: false,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        }
    }

    fn uncontrolled_text_form(id: &str, field_ids: &[&str]) -> quadraui::Form {
        quadraui::Form {
            id: WidgetId::new(id),
            fields: field_ids.iter().map(|f| text_input_field(f)).collect(),
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        }
    }

    /// Acceptance coverage for the "engine-owned by default" contract: an
    /// app that never sets `focused_field` at all (always `None`) still
    /// gets Tab-driven field focus, and typing afterward lands on the
    /// right field — the headless-driver counterpart of
    /// `quadraui-py`/`quadraui-node`'s own acceptance tests.
    #[test]
    fn uncontrolled_main_tree_form_tab_enters_the_first_field_and_typing_works() {
        use quadraui::FormEvent;

        let logic = FormLogic {
            form: uncontrolled_text_form("settings", &["a", "b", "c"]),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();

        // First Tab: nothing is focused yet anywhere, so the generic
        // app-level cycle (the form is its only stop) lands engine focus
        // on the form as a whole.
        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        driver.render();

        // Second Tab: `Self::tab_stops` now excludes the form (it holds
        // engine focus), so this one reaches `Self::handle_form_tab`
        // instead of the generic cycle -- moving onto the first field.
        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert!(
                dispatched.iter().any(|(ev, _)| matches!(
                    ev,
                    UiEvent::Form(_, FormEvent::FocusChanged { id }) if *id == WidgetId::new("a")
                )),
                "the second Tab should focus field \"a\": {:?}",
                dispatched
            );
        }
        driver.app().sink.borrow_mut().dispatched.clear();
        driver.render();

        // Typing now lands on "a" even though the app never echoed
        // `focused_field` back at all -- `None` is no longer a dead form.
        driver.dispatch(UiEvent::CharTyped('x'));
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(ev, _)| matches!(
                ev,
                UiEvent::Form(_, FormEvent::TextInputChanged { id, value })
                    if *id == WidgetId::new("a") && value == "x"
            )),
            "typing after Tab should land in field \"a\": {:?}",
            dispatched
        );
    }

    /// Tab/Shift-Tab moves between fields, then leaves the form once a
    /// cycle at either end has nowhere left to go inside it -- the last
    /// field's own next Tab must move engine focus onto the sibling
    /// button, not wrap back to the first field.
    #[test]
    fn tab_at_the_last_field_leaves_the_main_tree_form_for_the_next_app_level_stop() {
        let mut form = uncontrolled_text_form("settings", &["a", "b"]);
        // Simulates the form already being mid-navigation: "b" (the last
        // field) already has focus, same as if two Tabs had already
        // landed engine focus on the form and moved into its last field.
        form.focused_field = Some(WidgetId::new("b"));
        let logic = FormAndButtonLogic {
            form,
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            backend.set_focus(Some(WidgetId::new("settings")));
        }
        driver.render();

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let (backend, app) = driver.parts_mut();
        assert_eq!(
            backend.focus_manager().focused().copied(),
            Some(WidgetId::new("go")),
            "Tab at the last field must leave the form for the next app-level stop"
        );
        assert!(
            app.sink
                .borrow()
                .dispatched
                .iter()
                .any(|(ev, _)| matches!(ev, UiEvent::FocusChanged(Some(id)) if *id == WidgetId::new("go"))),
            "leaving the form must dispatch FocusChanged(\"go\"): {:?}",
            app.sink.borrow().dispatched
        );
    }

    /// Regression coverage for `quadraui::runtime::preprocess_event`'s own
    /// Tab/Shift-Tab step: it must not run its generic cross-widget cycle
    /// (and clear engine focus doing so) while a main-tree `"form"` extra
    /// holds focus, even when the form sits between *two* other app-level
    /// stops rather than next to just one. Before this is fixed,
    /// `preprocess_event` sees `app.tab_stops()` return the non-empty
    /// `[before, after]` (the form's own stop excluded while it holds
    /// focus), calls `FocusManager::sync_tab_order` on it, which clears
    /// focus because `"settings"` isn't in that list, and then
    /// `focus_next()` from `None` always lands on `stops[0]` (`"before"`)
    /// regardless of which end of the form Tab was pressed from or which
    /// sibling actually sits next to it in paint order — so this test
    /// fails against that implementation on every assertion below.
    #[test]
    fn uncontrolled_main_tree_form_tab_cycles_fields_then_leaves_for_the_correct_sibling() {
        use quadraui::FormEvent;

        let logic = FormBetweenTwoButtonsLogic {
            form: uncontrolled_text_form("settings", &["a", "b"]),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            backend.set_focus(Some(WidgetId::new("settings")));
        }
        driver.render();

        // (a) Tab with nothing focused inside the form yet enters the
        // first field, "a" -- not the generic cycle's "before" sibling.
        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        {
            let (backend, app) = driver.parts_mut();
            assert_eq!(
                backend.focus_manager().focused().copied(),
                Some(WidgetId::new("settings")),
                "the first Tab must stay inside the form, not leave it for a sibling"
            );
            assert!(
                app.sink.borrow().dispatched.iter().any(|(ev, _)| matches!(
                    ev,
                    UiEvent::Form(_, FormEvent::FocusChanged { id }) if *id == WidgetId::new("a")
                )),
                "the first Tab should focus field \"a\": {:?}",
                app.sink.borrow().dispatched
            );
        }
        driver.app().sink.borrow_mut().dispatched.clear();
        driver.render();

        // Tab again cycles within the form, a -> b.
        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        {
            let (backend, app) = driver.parts_mut();
            assert_eq!(
                backend.focus_manager().focused().copied(),
                Some(WidgetId::new("settings")),
                "Tab from field \"a\" must still stay inside the form"
            );
            assert!(
                app.sink.borrow().dispatched.iter().any(|(ev, _)| matches!(
                    ev,
                    UiEvent::Form(_, FormEvent::FocusChanged { id }) if *id == WidgetId::new("b")
                )),
                "the second Tab should move from \"a\" to \"b\": {:?}",
                app.sink.borrow().dispatched
            );
        }
        driver.app().sink.borrow_mut().dispatched.clear();
        driver.render();

        // (b) Tab at the last field, "b", leaves the form for "after" --
        // the sibling actually next to the form, not "before" (`stops[0]`).
        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        let (backend, app) = driver.parts_mut();
        assert_eq!(
            backend.focus_manager().focused().copied(),
            Some(WidgetId::new("after")),
            "Tab at the last field must leave the form for the \"after\" sibling"
        );
        assert!(
            app.sink
                .borrow()
                .dispatched
                .iter()
                .any(|(ev, _)| matches!(ev, UiEvent::FocusChanged(Some(id)) if *id == WidgetId::new("after"))),
            "leaving the form must dispatch FocusChanged(\"after\"): {:?}",
            app.sink.borrow().dispatched
        );
    }

    /// Shift-Tab is the mirror of the previous test: leaving the form at
    /// its *first* field moves engine focus to the "before" sibling, not
    /// "after". Also exercises `form_focus_at_boundary`'s own `None` case
    /// for Shift-Tab: entering with no field focused at all lands on the
    /// *last* field, "b", the same way plain Tab lands on the first --
    /// `FormController::move_focus` already resolves `None` this way for
    /// both directions, so neither end should be treated as a boundary.
    #[test]
    fn shift_tab_cycles_fields_backward_then_leaves_for_the_correct_sibling() {
        use quadraui::FormEvent;

        let logic = FormBetweenTwoButtonsLogic {
            form: uncontrolled_text_form("settings", &["a", "b"]),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            backend.set_focus(Some(WidgetId::new("settings")));
        }
        driver.render();

        let shift_tab = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
            repeat: false,
        };

        // Shift-Tab with nothing focused enters the *last* field, "b".
        driver.dispatch(shift_tab.clone());
        {
            let (backend, app) = driver.parts_mut();
            assert_eq!(
                backend.focus_manager().focused().copied(),
                Some(WidgetId::new("settings")),
                "the first Shift-Tab must stay inside the form, not leave it for a sibling"
            );
            assert!(
                app.sink.borrow().dispatched.iter().any(|(ev, _)| matches!(
                    ev,
                    UiEvent::Form(_, FormEvent::FocusChanged { id }) if *id == WidgetId::new("b")
                )),
                "the first Shift-Tab should focus field \"b\": {:?}",
                app.sink.borrow().dispatched
            );
        }
        driver.app().sink.borrow_mut().dispatched.clear();
        driver.render();

        // Shift-Tab at the first field, "b" -> "a", stays inside.
        driver.dispatch(shift_tab.clone());
        {
            let (_backend, app) = driver.parts_mut();
            assert!(
                app.sink.borrow().dispatched.iter().any(|(ev, _)| matches!(
                    ev,
                    UiEvent::Form(_, FormEvent::FocusChanged { id }) if *id == WidgetId::new("a")
                )),
                "the second Shift-Tab should move from \"b\" to \"a\": {:?}",
                app.sink.borrow().dispatched
            );
        }
        driver.app().sink.borrow_mut().dispatched.clear();
        driver.render();

        // Shift-Tab at the first field, "a", leaves the form for "before".
        driver.dispatch(shift_tab);
        let (backend, app) = driver.parts_mut();
        assert_eq!(
            backend.focus_manager().focused().copied(),
            Some(WidgetId::new("before")),
            "Shift-Tab at the first field must leave the form for the \"before\" sibling"
        );
        assert!(
            app.sink
                .borrow()
                .dispatched
                .iter()
                .any(|(ev, _)| matches!(ev, UiEvent::FocusChanged(Some(id)) if *id == WidgetId::new("before"))),
            "leaving the form must dispatch FocusChanged(\"before\"): {:?}",
            app.sink.borrow().dispatched
        );
    }

    /// Regression coverage for the `RefCell` double-borrow
    /// `InteractionEngine::handle`'s own form-key routing used to panic
    /// on: a `KeyPressed` landing on a focused `"form"` extra whose field
    /// has no text value borrows `Self::form_controllers` mutably to call
    /// `FormController::handle_event`, and — before the mutable borrow
    /// from that `if let` was scoped to end before this call —
    /// `Self::maybe_move_form_focus_vertically`'s own `Handled::Ignored`
    /// fallback borrowed the same `RefCell` immutably while the first
    /// borrow was still alive, an instant `BorrowError` panic. `Toggle`
    /// is one of several field kinds with no text value at all
    /// (`Button`/`Slider`/`ColorPicker` share the same shape); Down must
    /// move engine focus to the next field instead of panicking.
    #[test]
    fn down_on_a_focused_non_text_field_moves_to_the_next_field_without_panicking() {
        use quadraui::FormEvent;

        let mut form = uncontrolled_text_form("settings", &["a"]);
        form.fields.insert(0, toggle_field("toggle"));
        form.focused_field = Some(WidgetId::new("toggle"));
        let logic = FormLogic {
            form,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            backend.set_focus(Some(WidgetId::new("settings")));
        }
        driver.render();

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Down),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let app = driver.app();
        assert!(
            app.sink.borrow().dispatched.iter().any(|(ev, _)| matches!(
                ev,
                UiEvent::Form(_, FormEvent::FocusChanged { id }) if *id == WidgetId::new("a")
            )),
            "Down on the focused Toggle should move to field \"a\": {:?}",
            app.sink.borrow().dispatched
        );
    }

    /// Mirrors the previous test for the other half of the panic
    /// condition the reviewer called out: `focused_field: None` reaches
    /// the exact same `Handled::Ignored` fallback (`handle_text_op`
    /// bails out on `form.focused_field` being `None` before it ever
    /// looks at a field kind), so Down on a focused, uncontrolled form
    /// with nothing focused yet must not panic either — it focuses the
    /// first field, same as Tab does.
    #[test]
    fn down_on_a_focused_form_with_no_field_focused_yet_does_not_panic() {
        use quadraui::FormEvent;

        let logic = FormLogic {
            form: uncontrolled_text_form("settings", &["a", "b"]),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            backend.set_focus(Some(WidgetId::new("settings")));
        }
        driver.render();

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Down),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let app = driver.app();
        assert!(
            app.sink.borrow().dispatched.iter().any(|(ev, _)| matches!(
                ev,
                UiEvent::Form(_, FormEvent::FocusChanged { id }) if *id == WidgetId::new("a")
            )),
            "Down with nothing focused should focus the first field, \"a\": {:?}",
            app.sink.borrow().dispatched
        );
    }

    /// Regression coverage for the generic engine-focus ring's "one frame
    /// stale" window a `"form"` extra used to fall into: `tui::run::paint_frame`
    /// used to resolve `InteractionEngine::tab_stops`'s focus-ring rect
    /// *before* `app.render` ran, but `tab_stops` only excludes a `"form"`'s
    /// own stop (handing the ring off to `FormController`'s own per-field
    /// focus visuals instead) once `InteractionEngine::sync_forms` — called
    /// from `app.render` — has recorded that engine focus landed on it.
    /// The very first frame after `backend.set_focus` names the form, that
    /// record is still last frame's (not yet including this one), so the
    /// generic single-box ring painted the whole multi-row form rect,
    /// overwriting its first and last visible field rows with border
    /// glyphs. With more fields than fit the viewport, that made the first
    /// field ("name") disappear under the top border line, leaving the
    /// second field as the first thing actually visible — exactly the
    /// shape a scrolled-down viewport would show, with no scroll ever
    /// having happened.
    #[test]
    fn form_gaining_engine_focus_does_not_paint_a_whole_form_focus_ring_over_its_first_field() {
        let form = uncontrolled_text_form("settings", &["name", "b", "c", "d", "e", "f"]);
        let logic = FormLogic {
            form,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 6);
        driver.render();
        assert!(
            driver.screen_contains("name"),
            "the \"name\" field must be visible before the form has focus:\n{}",
            driver.screen()
        );

        {
            let (backend, _app) = driver.parts_mut();
            backend.set_focus(Some(WidgetId::new("settings")));
        }
        driver.render();
        assert!(
            driver.screen_contains("name"),
            "the \"name\" field must still be visible the very first frame \
             after the form gains engine focus -- the generic focus ring \
             must not overwrite it:\n{}",
            driver.screen()
        );
        assert!(
            !driver.screen().contains('╭'),
            "a \"form\" extra with engine focus must never paint the \
             generic single-box focus ring over its own multi-row content; \
             `FormController`'s own per-field focus visuals own that job:\n{}",
            driver.screen()
        );
    }

    /// An app that sets `focused_field` explicitly stays controlled: a
    /// click that would otherwise move engine-owned focus elsewhere must
    /// not change what the next render paints as focused, since the
    /// descriptor keeps pinning the same field every frame.
    #[test]
    fn controlled_main_tree_form_keeps_app_set_focus_despite_a_click() {
        let mut form = uncontrolled_text_form("settings", &["a", "b"]);
        form.focused_field = Some(WidgetId::new("a"));
        let logic = FormLogic {
            form,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();

        // Clicking field "b" still reports FocusChanged -- the app just
        // chooses not to act on it, same as any other controlled field.
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(1.0, 1.0),
            modifiers: Modifiers::default(),
        });
        driver.render();

        let resolved = driver
            .app()
            .engine
            .form_controllers
            .borrow()
            .get(&WidgetId::new("settings"))
            .and_then(|c| c.form().and_then(|f| f.focused_field));
        assert_eq!(
            resolved,
            Some(WidgetId::new("a")),
            "a controlled Form.focused_field must keep winning every frame, \
             regardless of a click the app's own descriptor never reacted to"
        );
    }

    /// A wheel `Scroll` landing inside a `"form"` extra's own rect must
    /// advance its cached [`quadraui::compose::FormController`]'s own
    /// `scroll_offset` — regression coverage for the `MouseUp`/`MouseMoved`/
    /// `Scroll` routing [`InteractionEngine::handle`] added alongside the
    /// `MouseDown` claim loop above it.
    #[test]
    fn wheel_scroll_over_a_form_advances_its_scroll_offset() {
        use quadraui::{FieldKind, FormField, ScrollDelta, StyledText};

        let fields = (0..10)
            .map(|i| FormField {
                id: WidgetId::new(format!("f{i}")),
                label: StyledText::plain(format!("Field{i}")),
                kind: FieldKind::Label,
                hint: StyledText::plain(""),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            })
            .collect();
        let form = quadraui::Form {
            id: WidgetId::new("settings"),
            fields,
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };

        let logic = FormLogic {
            form,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);

        driver.dispatch(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta { x: 0.0, y: -1.0 },
            position: Point::new(1.0, 1.0),
        });

        let (_backend, app) = driver.parts_mut();
        let offset = app
            .engine
            .form_controllers
            .borrow()
            .get(&WidgetId::new("settings"))
            .map(|fc| fc.scroll_offset());
        assert_eq!(offset, Some(1), "wheel scroll should advance scroll_offset");
    }

    /// A `MouseDown` on the button must round-trip through
    /// [`InteractionEngine::handle`] as click-to-focus (a `FocusChanged`
    /// dispatch), then the semantic `UiEvent::Form(id,
    /// FormEvent::ButtonClicked)` [`crate::semantic::button_clicked_event`]
    /// builds, then the raw click itself with its hit still attached — the
    /// semantic dispatch is additive, never a replacement, so a binding
    /// that already synthesizes its own click from the raw `MouseDown`
    /// keeps working unchanged. Proves the orchestration itself (not just
    /// paint/hit-test) is usable with no host-language binding in scope.
    #[test]
    fn click_on_a_button_focuses_it_then_dispatches_button_clicked_then_the_raw_click() {
        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 20, 5);

        let button_bounds = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    crate::hit::PaintedHit::Button { layout, .. } => Some(layout.bounds),
                    _ => None,
                })
                .expect("button should have painted a hit")
        };
        let click_point = Point::new(
            button_bounds.x + button_bounds.width / 2.0,
            button_bounds.y + button_bounds.height / 2.0,
        );

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: click_point,
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert_eq!(
            dispatched.len(),
            3,
            "expected a focus dispatch, the semantic ButtonClicked, and the raw click: {:?}",
            dispatched
        );
        assert!(
            matches!(dispatched[0].0, UiEvent::FocusChanged(Some(id)) if id == WidgetId::new("go")),
            "click-to-focus must dispatch FocusChanged first: {:?}",
            dispatched[0]
        );
        assert_eq!(
            dispatched[1].0,
            UiEvent::Form(
                WidgetId::new("go"),
                quadraui::FormEvent::ButtonClicked {
                    id: WidgetId::new("go")
                }
            ),
            "the semantic ButtonClicked must follow the focus dispatch: {:?}",
            dispatched[1]
        );
        assert!(
            matches!(dispatched[2].0, UiEvent::MouseDown { .. }),
            "the raw click must still follow, with its hit attached, for compatibility \
             with a binding that already synthesizes its own click from it: {:?}",
            dispatched[2]
        );
        assert!(
            dispatched[2].1.is_some(),
            "a button has no semantic click SUBSTITUTION (only an additive one), so the \
             raw click keeps its hit"
        );
    }

    /// Enter on a focused layout-tree `Button` round-trips through
    /// [`InteractionEngine::handle`] as the same semantic
    /// `UiEvent::Form(id, FormEvent::ButtonClicked)` a pointer click
    /// produces — the keyboard-activation half of this engine's `Button`
    /// coverage, exercised via [`crate::semantic::focus_scoped_key_event`]'s
    /// own `PaintedHit::Button` arm rather than
    /// [`InteractionEngine::handle`]'s `MouseDown`/`DoubleClick` routing.
    /// Unlike the pointer click, this fully replaces the raw `KeyPressed`
    /// — matching `Toolbar::activate_focused`'s own Enter/Space handling,
    /// which does the same.
    #[test]
    fn enter_on_a_focused_button_dispatches_button_clicked() {
        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 20, 5);
        {
            let (backend, _app) = driver.parts_mut();
            backend.set_focus(Some(WidgetId::new("go")));
        }

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert_eq!(
            dispatched,
            &[(
                UiEvent::Form(
                    WidgetId::new("go"),
                    quadraui::FormEvent::ButtonClicked {
                        id: WidgetId::new("go")
                    }
                ),
                None
            )],
            "Enter on a focused Button must dispatch exactly the semantic \
             ButtonClicked, replacing the raw KeyPressed: {:?}",
            dispatched
        );
    }

    /// Space does the same as Enter (see
    /// [`enter_on_a_focused_button_dispatches_button_clicked`]) — both are
    /// the chords [`quadraui::Button::handle_key`]'s own doc names as this
    /// primitive's activation keys.
    #[test]
    fn space_on_a_focused_button_dispatches_button_clicked() {
        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 20, 5);
        {
            let (backend, _app) = driver.parts_mut();
            backend.set_focus(Some(WidgetId::new("go")));
        }

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Char(' '),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert_eq!(
            dispatched,
            &[(
                UiEvent::Form(
                    WidgetId::new("go"),
                    quadraui::FormEvent::ButtonClicked {
                        id: WidgetId::new("go")
                    }
                ),
                None
            )],
            "Space on a focused Button must dispatch exactly the semantic \
             ButtonClicked, replacing the raw KeyPressed: {:?}",
            dispatched
        );
    }

    /// A `MouseMoved` landing on a painted button, then one moving off it,
    /// round-trips through [`InteractionEngine::handle`] as a hit-tested
    /// `UiEvent::MouseEntered` followed later by a `UiEvent::MouseLeft`,
    /// each carrying the button's own `HitDetail::ButtonBody` — the real
    /// hover signal a binding's own toolbar/tree/details-pane hover
    /// tooltip needs, synthesized the same way for every binding since it
    /// lives in this shared engine rather than each binding re-deriving
    /// hit-testing from the raw position itself.
    #[test]
    fn mouse_moved_onto_then_off_a_button_synthesizes_entered_then_left() {
        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 20, 5);

        let button_bounds = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    crate::hit::PaintedHit::Button { layout, .. } => Some(layout.bounds),
                    _ => None,
                })
                .expect("button should have painted a hit")
        };
        let inside = Point::new(
            button_bounds.x + button_bounds.width / 2.0,
            button_bounds.y + button_bounds.height / 2.0,
        );
        let outside = Point::new(button_bounds.x + button_bounds.width + 5.0, 0.0);

        // A move that lands nowhere painted synthesizes no hover event:
        // nothing was hovered before, nothing is hovered now — only the
        // raw `MouseMoved` itself reaches `sink`, same as every other
        // event this engine passes through unconditionally.
        driver.dispatch(UiEvent::MouseMoved {
            position: outside,
            buttons: quadraui::ButtonMask::default(),
        });
        assert_eq!(
            driver.app().sink.borrow().dispatched.len(),
            1,
            "a move with nothing under the cursor, before or after, synthesizes no hover event"
        );
        driver.app().sink.borrow_mut().dispatched.clear();

        driver.dispatch(UiEvent::MouseMoved {
            position: inside,
            buttons: quadraui::ButtonMask::default(),
        });
        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert_eq!(
                dispatched.len(),
                2,
                "expected the synthesized MouseEntered followed by the raw MouseMoved: {dispatched:?}"
            );
            assert_eq!(
                dispatched[0],
                (
                    UiEvent::MouseEntered {
                        widget: WidgetId::new("go")
                    },
                    Some(HitDetail::ButtonBody)
                ),
                "entering the button must dispatch MouseEntered with its hit attached"
            );
            assert!(
                matches!(dispatched[1].0, UiEvent::MouseMoved { .. }),
                "the raw move must still follow: {:?}",
                dispatched[1]
            );
        }
        driver.app().sink.borrow_mut().dispatched.clear();

        driver.dispatch(UiEvent::MouseMoved {
            position: outside,
            buttons: quadraui::ButtonMask::default(),
        });
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert_eq!(
            dispatched.len(),
            2,
            "expected the synthesized MouseLeft followed by the raw MouseMoved: {dispatched:?}"
        );
        assert_eq!(
            dispatched[0],
            (
                UiEvent::MouseLeft {
                    widget: WidgetId::new("go")
                },
                Some(HitDetail::ButtonBody)
            ),
            "leaving the button must dispatch MouseLeft with its old hit attached"
        );
        assert!(
            matches!(dispatched[1].0, UiEvent::MouseMoved { .. }),
            "the raw move must still follow: {:?}",
            dispatched[1]
        );
    }

    /// A message dialog opened via [`InteractionEngine::open_message_dialog`]
    /// intercepts an Enter keypress before any of the rest of
    /// [`InteractionEngine::handle`]'s own routing runs (the painted
    /// `Button` behind it never sees the key), dispatches
    /// `UiEvent::MessageDialogResolved` with the default button's id, and
    /// pops its [`quadraui::ModalStack`] entry on resolution — the same
    /// "owns every event while open, closes cleanly on resolve" contract
    /// this module's own context-menu coverage already proves for
    /// `ctx_menu`. The click path (hit-testing the dialog's own button
    /// layout) is covered black-box, through the real binding boundary, by
    /// `quadraui-py`'s `show_message_dialog_then_clicking_a_button_emits_message_dialog_resolved`.
    #[test]
    fn open_message_dialog_then_pressing_enter_resolves_the_default_button_and_pops_the_modal_stack(
    ) {
        use quadraui::backend::{MessageDialogButton, MessageDialogOptions};
        use quadraui::{Key, NamedKey};

        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);

        let opts = MessageDialogOptions {
            title: "Unsaved changes".to_string(),
            body: "Save before closing?".to_string(),
            buttons: vec![
                MessageDialogButton {
                    id: WidgetId::new("save"),
                    label: "Save".to_string(),
                    is_default: true,
                    is_cancel: false,
                    spec: None,
                },
                MessageDialogButton {
                    id: WidgetId::new("cancel"),
                    label: "Cancel".to_string(),
                    is_default: false,
                    is_cancel: true,
                    spec: None,
                },
            ],
            severity: None,
        };
        {
            let (backend, app) = driver.parts_mut();
            app.engine.open_message_dialog(opts, backend);
            assert!(
                backend.modal_stack_handle().borrow().top().is_some(),
                "opening the dialog must push a ModalStack entry"
            );
        }
        driver.render();

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert_eq!(
                dispatched.len(),
                1,
                "resolving the dialog dispatches exactly one event: {:?}",
                dispatched
            );
            assert!(
                matches!(
                    dispatched[0].0,
                    UiEvent::MessageDialogResolved(id) if id == WidgetId::new("save")
                ),
                "expected MessageDialogResolved(save): {:?}",
                dispatched[0]
            );
        }

        let (backend, app) = driver.parts_mut();
        assert!(
            !app.engine.dialog_open(),
            "the dialog must close once resolved"
        );
        assert!(
            backend.modal_stack_handle().borrow().top().is_none(),
            "resolving the dialog must pop its ModalStack entry"
        );
    }

    /// Acceptance coverage for a content-bearing modal dialog: a
    /// [`FormDialogController`] hosting a `text_input` field emits the
    /// normal `UiEvent::Form` event for typing, and Tab never escapes
    /// the modal to the main tree's own button underneath it —
    /// exercised through the real `TuiDriver::dispatch` path (not a
    /// direct `engine.handle` call), so `quadraui::runtime::preprocess_event`'s
    /// own global Tab-cycle is actually in the loop, the same way a real
    /// binding's runner would drive it. See [`InteractionEngine::tab_stops`]'s
    /// own doc for why trapping Tab needs this method's cooperation, not
    /// just [`FormDialogController`]'s internal handling. A `switch`
    /// (`FieldKind::Toggle`) field's own normal-event path is covered by
    /// the popover test below instead — computing a click position for
    /// it needs `FormController::field_rect`, a method this test's own
    /// `FormDialogController` has no equivalent public accessor for.
    #[test]
    fn open_form_dialog_with_text_input_and_toggle_traps_tab_and_emits_normal_form_events() {
        use quadraui::compose::FormDialogController;
        use quadraui::{DialogButton, FieldKind, Form, FormEvent, FormField, StyledText};

        fn field(id: &str, kind: FieldKind) -> FormField {
            FormField {
                id: WidgetId::new(id),
                label: StyledText::plain(id),
                kind,
                hint: StyledText::default(),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }
        }

        let form = Form {
            id: WidgetId::new("form:prefs"),
            fields: vec![
                field(
                    "name",
                    FieldKind::TextInput {
                        value: String::new(),
                        placeholder: String::new(),
                        cursor: Some(0),
                        selection_anchor: None,
                    },
                ),
                field("flag", FieldKind::Toggle { value: false }),
            ],
            focused_field: Some(WidgetId::new("name")),
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };

        let mut dialog = FormDialogController::new("prefs", "Preferences").with_buttons(vec![
            DialogButton {
                id: WidgetId::new("cancel"),
                label: "Cancel".into(),
                is_default: false,
                is_cancel: true,
                tint: None,
                spec: None,
            },
            DialogButton {
                id: WidgetId::new("ok"),
                label: "OK".into(),
                is_default: true,
                is_cancel: false,
                tint: None,
                spec: None,
            },
        ]);
        dialog.set_form(form);

        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 60, 24);
        {
            let (backend, app) = driver.parts_mut();
            app.engine.open_form_dialog(dialog, backend);
        }
        driver.render();

        assert!(
            driver.app().engine.tab_stops().is_empty(),
            "tab_stops must report no main-tree stops while a form-dialog \
             is open, or `preprocess_event`'s global Tab-cycle would reach \
             straight past the modal to whatever is painted underneath it"
        );

        // Typing lands in the already-focused "name" field as a normal
        // `UiEvent::Form(.., TextInputChanged)`.
        driver.dispatch(UiEvent::CharTyped('x'));
        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert!(
                dispatched.iter().any(|(ev, _)| matches!(
                    ev,
                    UiEvent::Form(_, FormEvent::TextInputChanged { id, value })
                        if *id == WidgetId::new("name") && value == "x"
                )),
                "typing should emit a normal Form(TextInputChanged) event: {:?}",
                dispatched
            );
        }
        driver.app().sink.borrow_mut().dispatched.clear();

        // Tab moves focus from "name" to "flag" -- it must never reach
        // past the dialog's own fields/buttons to the main tree's "go"
        // button, which is what a failure of the `tab_stops` trap above
        // would look like end-to-end.
        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert!(
                !dispatched.iter().any(|(ev, _)| matches!(
                    ev,
                    UiEvent::FocusChanged(Some(id)) if *id == WidgetId::new("go")
                )),
                "Tab must stay inside the modal, never focus the main \
                 tree's own button: {:?}",
                dispatched
            );
        }
        // Positive half of the Tab assertion above: not only did focus
        // fail to reach "go", it actually advanced inside the dialog's
        // own form, from "name" to "flag" -- a Tab that did nothing at
        // all (or was swallowed outright) would pass the negative check
        // above identically, so this is the part that actually proves
        // the trap works end-to-end rather than merely not leaking.
        assert_eq!(
            driver
                .app_mut()
                .engine
                .form_dialog_mut()
                .and_then(|d| d.form())
                .and_then(|f| f.focused_field),
            Some(WidgetId::new("flag")),
            "Tab should advance focus from \"name\" to \"flag\" inside the \
             dialog's own form"
        );

        assert!(
            driver.app().engine.form_dialog_open(),
            "the dialog must still be open -- nothing above should have closed it"
        );
    }

    /// Acceptance coverage for a content-bearing popover: a
    /// [`Self::show_popover`] opened anchored to a button's own painted
    /// rect emits the normal `UiEvent::Form` event for typing in a
    /// `text_input` field and for clicking a `switch` (`FieldKind::Toggle`)
    /// field, and is dismissed by a `MouseDown` landing outside its
    /// bounds -- popping its [`quadraui::ModalStack`] entry the same way
    /// [`Self::close_message_dialog`]'s own test already proves for a
    /// message dialog. The toggle field's click position is computed via
    /// a scratch [`FormController`] (same `form`, same
    /// [`FormController::field_rect`] the real popover's own embedded
    /// controller resolves a click against) rather than a hardcoded
    /// coordinate.
    #[test]
    fn show_popover_with_a_text_input_and_toggle_emits_normal_form_events_and_is_dismissed_by_an_outside_click(
    ) {
        use quadraui::compose::FormController;
        use quadraui::{Anchor, FieldKind, Float, FloatMeasure, Form, FormEvent, FormField, Side};

        fn field(id: &str, kind: FieldKind) -> FormField {
            FormField {
                id: WidgetId::new(id),
                label: quadraui::StyledText::plain(id),
                kind,
                hint: quadraui::StyledText::default(),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }
        }

        let form = Form {
            id: WidgetId::new("form:popover"),
            fields: vec![
                field(
                    "note",
                    FieldKind::TextInput {
                        value: String::new(),
                        placeholder: String::new(),
                        cursor: Some(0),
                        selection_anchor: None,
                    },
                ),
                field("flag", FieldKind::Toggle { value: false }),
            ],
            focused_field: Some(WidgetId::new("note")),
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };

        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 60, 24);
        driver.render();
        let button_rect = {
            let hit_map = driver.app().engine.hit_map();
            hit_map
                .iter()
                .find_map(|hit| match hit {
                    PaintedHit::Button { id, layout } if *id == WidgetId::new("go") => {
                        Some(layout.bounds)
                    }
                    _ => None,
                })
                .expect("the background button should have painted a hit")
        };

        let float = Float::new(
            WidgetId::new("popover:note"),
            Anchor::new(button_rect, Side::Bottom),
        );
        let measure = FloatMeasure::new(24.0, 6.0);
        let content_bounds = {
            let (backend, app) = driver.parts_mut();
            app.engine
                .show_popover(float.clone(), measure, form.clone(), backend);
            app.engine
                .float_layout(backend, &float, measure)
                .content_bounds
        };
        driver.render();
        assert!(
            driver.app().engine.tab_stops().is_empty(),
            "tab_stops must report no main-tree stops while the popover is open"
        );

        // Typing lands in the already-focused "note" field.
        driver.dispatch(UiEvent::CharTyped('x'));
        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert!(
                dispatched.iter().any(|(ev, _)| matches!(
                    ev,
                    UiEvent::Form(_, FormEvent::TextInputChanged { id, value })
                        if *id == WidgetId::new("note") && value == "x"
                )),
                "typing should emit a normal Form(TextInputChanged) event: {:?}",
                dispatched
            );
        }
        driver.app().sink.borrow_mut().dispatched.clear();

        // Click the "flag" toggle, at the position a scratch
        // `FormController` (built from the same `form`) resolves it to.
        let flag_rect = {
            let mut scratch = FormController::new("scratch".to_string());
            scratch.set_form(form);
            let (backend, _) = driver.parts_mut();
            scratch
                .field_rect(WidgetId::new("flag"), backend, content_bounds)
                .expect("the toggle field should have a resolved rect")
        };
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(
                flag_rect.x + flag_rect.width * 0.5,
                flag_rect.y + flag_rect.height * 0.5,
            ),
            modifiers: Modifiers::default(),
        });
        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert!(
                dispatched.iter().any(|(ev, _)| matches!(
                    ev,
                    UiEvent::Form(_, FormEvent::ToggleChanged { id, value: true })
                        if *id == WidgetId::new("flag")
                )),
                "clicking the toggle should emit a normal Form(ToggleChanged) \
                 event: {:?}",
                dispatched
            );
        }

        // Well outside the popover's own box (and the background button):
        // the viewport's own bottom-right corner, derived from the real
        // backend rather than a bare literal that `float`'s own anchoring
        // (or the driver's own size above) could silently stop covering.
        let outside = {
            let (backend, _) = driver.parts_mut();
            let viewport = viewport_rect(backend);
            Point::new(viewport.width - 1.0, viewport.height - 1.0)
        };
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: outside,
            modifiers: Modifiers::default(),
        });

        let (backend, app) = driver.parts_mut();
        assert!(
            backend.modal_stack_handle().borrow().top().is_none(),
            "an outside click must dismiss the popover, popping its \
             ModalStack entry"
        );
        assert!(
            !app.engine.tab_stops().is_empty(),
            "once the popover is gone, `tab_stops` should report the main \
             tree's own stops again"
        );
    }

    /// Acceptance coverage for [`InteractionEngine::show_popover_layout`]:
    /// an arbitrary `LayoutNode` subtree — a `Column` holding a `"form"`
    /// leaf *and* a `Button` leaf, not just a single bare `Form` the way
    /// [`InteractionEngine::show_popover`]'s own test above exercises —
    /// painted into a popover via [`crate::render::paint_layout`], whose
    /// nested leaves both receive focus/click/keyboard routing the same
    /// shape the main tree's own `"form"`/`Button` leaves would produce.
    #[test]
    fn show_popover_layout_with_a_nested_form_and_button_emits_normal_events_and_is_dismissed_by_an_outside_click(
    ) {
        use quadraui::{Anchor, FieldKind, Float, FloatMeasure, Form, FormEvent, FormField, Side};

        fn field(id: &str, kind: FieldKind) -> FormField {
            FormField {
                id: WidgetId::new(id),
                label: quadraui::StyledText::plain(id),
                kind,
                hint: quadraui::StyledText::default(),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }
        }

        let form = Form {
            id: WidgetId::new("form:popover-layout"),
            fields: vec![field(
                "note",
                FieldKind::TextInput {
                    value: String::new(),
                    placeholder: String::new(),
                    cursor: Some(0),
                    selection_anchor: None,
                },
            )],
            focused_field: Some(WidgetId::new("note")),
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };

        let content = LayoutNode::Column(LayoutContainer::new(
            vec![
                LayoutNode::Leaf(LayoutLeaf {
                    id: "note-form".to_string(),
                    descriptor: LeafDescriptor::Extra(Rc::new(form)),
                    scroll_px: None,
                }),
                LayoutNode::Leaf(LayoutLeaf {
                    id: "submit".to_string(),
                    descriptor: LeafDescriptor::Button(Button::new(
                        WidgetId::new("submit"),
                        "Submit",
                    )),
                    scroll_px: None,
                }),
            ],
            Spacing::default(),
        ));

        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 60, 24);
        driver.render();
        let button_rect = {
            let hit_map = driver.app().engine.hit_map();
            hit_map
                .iter()
                .find_map(|hit| match hit {
                    PaintedHit::Button { id, layout } if *id == WidgetId::new("go") => {
                        Some(layout.bounds)
                    }
                    _ => None,
                })
                .expect("the background button should have painted a hit")
        };

        let float = Float::new(
            WidgetId::new("popover:layout"),
            Anchor::new(button_rect, Side::Bottom),
        );
        let measure = FloatMeasure::new(24.0, 6.0);
        {
            let (backend, app) = driver.parts_mut();
            app.engine
                .show_popover_layout(float.clone(), measure, content, backend);
        }
        driver.render();

        // The popover's own two leaves -- the "note-form" extra and the
        // "submit" button -- are the only tab stops while it's open (the
        // leaf's own id, same as a main-tree `"form"` leaf's tab stop is
        // keyed by its own leaf id rather than any one field inside it);
        // the main tree's own "go" button must not appear.
        let stops = driver.app().engine.tab_stops();
        assert!(
            stops
                .iter()
                .any(|(id, _)| *id == WidgetId::new("note-form")),
            "the popover's own form leaf should be a tab stop: {:?}",
            stops
        );
        let submit_rect = stops
            .iter()
            .find_map(|(id, rect)| (*id == WidgetId::new("submit")).then_some(*rect))
            .expect("the popover's own button should be a tab stop");
        assert!(
            !stops.iter().any(|(id, _)| *id == WidgetId::new("go")),
            "the main tree's own button must not be a tab stop while the \
             popover is open: {:?}",
            stops
        );

        // Typing lands in the already-focused "note" field, as a normal
        // `UiEvent::Form` event -- the same shape a main-tree "form" leaf
        // already emits.
        driver.dispatch(UiEvent::CharTyped('x'));
        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert!(
                dispatched.iter().any(|(ev, _)| matches!(
                    ev,
                    UiEvent::Form(_, FormEvent::TextInputChanged { id, value })
                        if *id == WidgetId::new("note") && value == "x"
                )),
                "typing should emit a normal Form(TextInputChanged) event: {:?}",
                dispatched
            );
        }
        driver.app().sink.borrow_mut().dispatched.clear();

        // Clicking the "submit" button click-focuses it (a `FocusChanged`
        // ahead of the click, same convention `Self::handle` uses for the
        // main tree) and emits the normal `button_clicked_event` shape.
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(
                submit_rect.x + submit_rect.width * 0.5,
                submit_rect.y + submit_rect.height * 0.5,
            ),
            modifiers: Modifiers::default(),
        });
        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert!(
                dispatched.iter().any(|(ev, _)| matches!(
                    ev,
                    UiEvent::FocusChanged(Some(id)) if *id == WidgetId::new("submit")
                )),
                "clicking the button should click-focus it first: {:?}",
                dispatched
            );
            assert!(
                dispatched.iter().any(|(ev, _)| matches!(
                    ev,
                    UiEvent::Form(id, FormEvent::ButtonClicked { id: inner })
                        if *id == WidgetId::new("submit") && *inner == WidgetId::new("submit")
                )),
                "clicking the button should emit the normal button_clicked_event: {:?}",
                dispatched
            );
        }

        // Well outside the popover's own box (and the background button).
        let outside = {
            let (backend, _) = driver.parts_mut();
            let viewport = viewport_rect(backend);
            Point::new(viewport.width - 1.0, viewport.height - 1.0)
        };
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: outside,
            modifiers: Modifiers::default(),
        });

        let (backend, app) = driver.parts_mut();
        assert!(
            backend.modal_stack_handle().borrow().top().is_none(),
            "an outside click must dismiss the popover, popping its \
             ModalStack entry"
        );
        assert!(
            !app.engine.tab_stops().is_empty(),
            "once the popover is gone, `tab_stops` should report the main \
             tree's own stops again"
        );
    }

    /// A `ListView` nested in a [`InteractionEngine::show_popover_layout`]
    /// popover routes clicks and focused keys through exactly the same
    /// [`crate::semantic::semantic_event_for_hit`]/
    /// [`crate::semantic::focus_scoped_key_event`] pair the main tree uses
    /// — the "every leaf kind, not just `Button`/`"form"`" half of a
    /// content-bearing overlay.
    #[test]
    fn show_popover_layout_routes_clicks_and_keys_for_a_nested_list_leaf() {
        use quadraui::{
            Anchor, Float, FloatMeasure, Key, ListItem, ListView, ListViewEvent, NamedKey, Side,
            StyledText,
        };

        let list = ListView {
            id: WidgetId::new("recent"),
            title: None,
            items: (0..3)
                .map(|i| ListItem {
                    text: StyledText::plain(format!("row {i}")),
                    icon: None,
                    detail: None,
                    decoration: Default::default(),
                })
                .collect(),
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        };
        let content = LayoutNode::Leaf(LayoutLeaf {
            id: "recent".to_string(),
            descriptor: LeafDescriptor::List(list),
            scroll_px: None,
        });

        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 60, 24);
        driver.render();
        let button_rect = {
            let hit_map = driver.app().engine.hit_map();
            hit_map
                .iter()
                .find_map(|hit| match hit {
                    PaintedHit::Button { id, layout } if *id == WidgetId::new("go") => {
                        Some(layout.bounds)
                    }
                    _ => None,
                })
                .expect("the background button should have painted a hit")
        };
        let float = Float::new(
            WidgetId::new("popover:recent"),
            Anchor::new(button_rect, Side::Bottom),
        );
        let measure = FloatMeasure::new(24.0, 6.0);
        {
            let (backend, app) = driver.parts_mut();
            app.engine
                .show_popover_layout(float, measure, content, backend);
        }
        driver.render();

        // The list is the popover's only tab stop, so rendering it seeded
        // keyboard focus onto it -- derive the row rect from the stop
        // rather than hardcoding a coordinate.
        let stops = driver.app().engine.tab_stops();
        let list_rect = stops
            .iter()
            .find_map(|(id, rect)| (*id == WidgetId::new("recent")).then_some(*rect))
            .expect("the popover's own list leaf should be a tab stop");

        // Click the second row: one line below the list's own top edge.
        let line_height = {
            let (backend, _) = driver.parts_mut();
            backend.measure().line_height
        };
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(list_rect.x + 1.0, list_rect.y + line_height * 1.5),
            modifiers: Modifiers::default(),
        });
        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert!(
                dispatched.iter().any(|(ev, _)| matches!(
                    ev,
                    UiEvent::List(id, ListViewEvent::SelectionChanged { idx: 1 })
                        if *id == WidgetId::new("recent")
                )),
                "clicking a row inside the popover should emit the normal \
                 List(SelectionChanged) event: {:?}",
                dispatched
            );
        }
        driver.app().sink.borrow_mut().dispatched.clear();

        // Enter on the focused list activates its selected row, the same
        // shape `focus_scoped_key_event` produces for a main-tree list.
        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert!(
                dispatched.iter().any(|(ev, _)| matches!(
                    ev,
                    UiEvent::List(id, ListViewEvent::ItemActivated { .. })
                        if *id == WidgetId::new("recent")
                )),
                "Enter on the focused list should emit List(ItemActivated): {:?}",
                dispatched
            );
        }
    }

    /// Acceptance coverage for [`InteractionEngine::open_layout_dialog`]:
    /// a modal dialog whose body hosts a `LayoutNode` subtree (a `"form"`
    /// leaf with a text input and a toggle, plus a `Button` leaf). Typing
    /// and toggling inside it emit the normal semantic events, Tab stays
    /// inside the modal (reaching the dialog's own buttons but never the
    /// main tree's), Escape resolves the dialog to its cancel button, and
    /// a window-lifecycle event still reaches the app while the modal is
    /// up.
    #[test]
    fn open_layout_dialog_with_nested_form_content_traps_tab_and_emits_normal_events() {
        use quadraui::compose::FormController;
        use quadraui::{
            DialogButton, FieldKind, Form, FormEvent, FormField, Key, NamedKey, StyledText,
        };

        use super::DialogContentSize;

        fn field(id: &str, kind: FieldKind) -> FormField {
            FormField {
                id: WidgetId::new(id),
                label: StyledText::plain(id),
                kind,
                hint: StyledText::default(),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }
        }

        let form = Form {
            id: WidgetId::new("form:dialog-layout"),
            fields: vec![
                field(
                    "name",
                    FieldKind::TextInput {
                        value: String::new(),
                        placeholder: String::new(),
                        cursor: Some(0),
                        selection_anchor: None,
                    },
                ),
                field("flag", FieldKind::Toggle { value: false }),
            ],
            focused_field: Some(WidgetId::new("name")),
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };

        let content = LayoutNode::Column(LayoutContainer::new(
            vec![
                LayoutNode::Leaf(LayoutLeaf {
                    id: "prefs-form".to_string(),
                    descriptor: LeafDescriptor::Extra(Rc::new(form.clone())),
                    scroll_px: None,
                }),
                LayoutNode::Leaf(LayoutLeaf {
                    id: "reset".to_string(),
                    descriptor: LeafDescriptor::Button(Button::new(
                        WidgetId::new("reset"),
                        "Reset",
                    )),
                    scroll_px: None,
                }),
            ],
            Spacing::default(),
        ));

        let dialog = quadraui::Dialog {
            id: WidgetId::new("dialog:prefs"),
            title: StyledText::plain("Preferences"),
            body: Vec::new(),
            buttons: vec![
                DialogButton {
                    id: WidgetId::new("save"),
                    label: "Save".to_string(),
                    is_default: true,
                    is_cancel: false,
                    tint: None,
                    spec: None,
                },
                DialogButton {
                    id: WidgetId::new("cancel"),
                    label: "Cancel".to_string(),
                    is_default: false,
                    is_cancel: true,
                    tint: None,
                    spec: None,
                },
            ],
            severity: None,
            vertical_buttons: false,
            table: None,
            input: None,
        };

        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 60, 24);
        driver.render();
        {
            let (backend, app) = driver.parts_mut();
            app.engine
                .open_layout_dialog(dialog, content, DialogContentSize::default(), backend);
            assert!(
                app.engine.layout_dialog_open(),
                "the dialog should report itself open"
            );
        }
        driver.render();

        // Tab stops: the subtree's own leaves plus the dialog's buttons,
        // and nothing from the main tree -- Tab can never leave the modal.
        let stops = driver.app().engine.tab_stops();
        for expected in ["prefs-form", "reset", "save", "cancel"] {
            assert!(
                stops.iter().any(|(id, _)| *id == WidgetId::new(expected)),
                "{expected} should be a tab stop while the dialog is open: {stops:?}"
            );
        }
        assert!(
            !stops.iter().any(|(id, _)| *id == WidgetId::new("go")),
            "the main tree's own button must not be a tab stop while the \
             dialog is open: {stops:?}"
        );

        // Typing lands in the already-focused "name" field.
        driver.dispatch(UiEvent::CharTyped('a'));
        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert!(
                dispatched.iter().any(|(ev, _)| matches!(
                    ev,
                    UiEvent::Form(_, FormEvent::TextInputChanged { id, value })
                        if *id == WidgetId::new("name") && value == "a"
                )),
                "typing inside the dialog should emit a normal \
                 Form(TextInputChanged) event: {:?}",
                dispatched
            );
        }
        driver.app().sink.borrow_mut().dispatched.clear();

        // Toggling the "flag" switch, at the position a scratch
        // `FormController` resolves it to inside the form leaf's own rect.
        let form_rect = stops
            .iter()
            .find_map(|(id, rect)| (*id == WidgetId::new("prefs-form")).then_some(*rect))
            .expect("the form leaf should be a tab stop");
        let flag_rect = {
            let mut scratch = FormController::new("scratch".to_string());
            scratch.set_form(form);
            let (backend, _) = driver.parts_mut();
            scratch
                .field_rect(WidgetId::new("flag"), backend, form_rect)
                .expect("the toggle field should have a resolved rect")
        };
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(
                flag_rect.x + flag_rect.width * 0.5,
                flag_rect.y + flag_rect.height * 0.5,
            ),
            modifiers: Modifiers::default(),
        });
        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert!(
                dispatched.iter().any(|(ev, _)| matches!(
                    ev,
                    UiEvent::Form(_, FormEvent::ToggleChanged { id, value: true })
                        if *id == WidgetId::new("flag")
                )),
                "toggling inside the dialog should emit a normal \
                 Form(ToggleChanged) event: {:?}",
                dispatched
            );
        }
        driver.app().sink.borrow_mut().dispatched.clear();

        // A window-lifecycle event still reaches the app while the modal
        // is up -- a modal traps *input*, not the window's own events.
        driver.dispatch(UiEvent::WindowResized {
            viewport: quadraui::Viewport::new(50.0, 20.0, 1.0),
        });
        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert!(
                dispatched
                    .iter()
                    .any(|(ev, _)| matches!(ev, UiEvent::WindowResized { .. })),
                "a WindowResized must still reach the app with a modal open: {:?}",
                dispatched
            );
        }
        driver.app().sink.borrow_mut().dispatched.clear();

        // Escape resolves the dialog to its cancel button and closes it.
        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert!(
                dispatched.iter().any(|(ev, _)| matches!(
                    ev,
                    UiEvent::MessageDialogResolved(id) if *id == WidgetId::new("cancel")
                )),
                "Escape should resolve the dialog to its cancel button: {:?}",
                dispatched
            );
        }
        let (backend, app) = driver.parts_mut();
        assert!(
            !app.engine.layout_dialog_open(),
            "resolving the dialog must close it"
        );
        assert!(
            backend.modal_stack_handle().borrow().top().is_none(),
            "closing the dialog must pop its ModalStack entry"
        );
    }

    /// Opening a message dialog while a layout dialog is already open
    /// replaces it cleanly: the two share [`Overlay::rank`], so the
    /// layout dialog has to be closed (and its [`quadraui::ModalStack`]
    /// entry popped, and the keyboard focus it seeded onto its own
    /// content released) rather than silently dropped out of the overlay
    /// stack by [`InteractionEngine::push_overlay`]'s "same rank, same
    /// slot" rule.
    #[test]
    fn opening_a_message_dialog_over_a_layout_dialog_pops_its_modal_entry_and_focus() {
        use quadraui::backend::{MessageDialogButton, MessageDialogOptions};
        use quadraui::{DialogButton, StyledText};

        use super::DialogContentSize;

        let content = LayoutNode::Leaf(LayoutLeaf {
            id: "apply".to_string(),
            descriptor: LeafDescriptor::Button(Button::new(WidgetId::new("apply"), "Apply")),
            scroll_px: None,
        });
        let dialog = quadraui::Dialog {
            id: WidgetId::new("dialog:prefs"),
            title: StyledText::plain("Preferences"),
            body: Vec::new(),
            buttons: vec![DialogButton {
                id: WidgetId::new("close"),
                label: "Close".to_string(),
                is_default: true,
                is_cancel: true,
                tint: None,
                spec: None,
            }],
            severity: None,
            vertical_buttons: false,
            table: None,
            input: None,
        };

        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 60, 24);
        driver.render();
        {
            let (backend, app) = driver.parts_mut();
            app.engine
                .open_layout_dialog(dialog, content, DialogContentSize::default(), backend);
        }
        driver.render();
        {
            let (backend, _) = driver.parts_mut();
            assert_eq!(
                backend.focus_manager().focused().copied(),
                Some(WidgetId::new("apply")),
                "opening the layout dialog should seed focus onto its own content"
            );
        }

        let opts = MessageDialogOptions {
            title: "Heads up".to_string(),
            body: "Something happened.".to_string(),
            buttons: vec![MessageDialogButton {
                id: WidgetId::new("ok"),
                label: "OK".to_string(),
                is_default: true,
                is_cancel: false,
                spec: None,
            }],
            severity: None,
        };
        {
            let (backend, app) = driver.parts_mut();
            app.engine.open_message_dialog(opts, backend);
        }

        let (backend, app) = driver.parts_mut();
        assert!(
            !app.engine.layout_dialog_open(),
            "the layout dialog must be closed, not left in the overlay stack"
        );
        assert_eq!(
            backend.modal_stack_handle().borrow().top().map(|e| e.id),
            Some(WidgetId::new("message_dialog")),
            "only the message dialog's own ModalStack entry should remain"
        );
        assert_eq!(
            backend.focus_manager().focused().copied(),
            None,
            "the closed dialog's content must not keep keyboard focus"
        );
    }

    /// Opening a message dialog while a context-menu popup is already
    /// open closes the popup: paint order puts the dialog on top
    /// ([`Overlay::Dialog`]'s own [`Overlay::rank`] is higher than
    /// [`Overlay::ContextMenu`]'s), and [`InteractionEngine::handle`]
    /// checks the dialog first, so a popup left open underneath would be
    /// unreachable input it still held precedence over.
    #[test]
    fn open_message_dialog_closes_an_open_context_menu_popup() {
        use quadraui::backend::{MessageDialogButton, MessageDialogOptions};
        use quadraui::{ContextMenuItem, Key, NamedKey};

        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);

        {
            let (backend, app) = driver.parts_mut();
            let items = vec![ContextMenuItem {
                id: Some(WidgetId::new("copy")),
                label: quadraui::types::StyledText::plain("Copy"),
                ..Default::default()
            }];
            app.engine
                .open_context_menu(items, Point::new(1.0, 1.0), backend);
            assert!(
                app.engine.context_menu_open(),
                "the popup must be open before the dialog opens"
            );
        }

        let opts = MessageDialogOptions {
            title: "Heads up".to_string(),
            body: "Something happened.".to_string(),
            buttons: vec![MessageDialogButton {
                id: WidgetId::new("ok"),
                label: "OK".to_string(),
                is_default: true,
                is_cancel: false,
                spec: None,
            }],
            severity: None,
        };
        {
            let (backend, app) = driver.parts_mut();
            app.engine.open_message_dialog(opts, backend);
            assert!(
                !app.engine.context_menu_open(),
                "opening the dialog must close the popup painted underneath it"
            );
        }
        driver.render();

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert_eq!(
            dispatched.len(),
            1,
            "Enter must resolve the dialog, not reach the already-closed popup: {:?}",
            dispatched
        );
        assert!(
            matches!(
                dispatched[0].0,
                UiEvent::MessageDialogResolved(id) if id == WidgetId::new("ok")
            ),
            "expected MessageDialogResolved(ok), not a context-menu event: {:?}",
            dispatched[0]
        );
    }

    /// A `ContextMenuItem::submenu` opened via [`InteractionEngine::open_context_menu`]
    /// — the entry point every binding's own `show_context_menu` routes
    /// through — opens by Right arrow and reports the activated nested
    /// leaf's own `WidgetId` regardless of nesting depth, exercised
    /// end-to-end through [`TuiDriver`] exactly as a Python/Node caller's
    /// `KeyPressed` events would arrive.
    #[test]
    fn open_context_menu_right_arrow_opens_submenu_and_enter_activates_nested_item() {
        use quadraui::ContextMenuItem;

        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);

        {
            let (backend, app) = driver.parts_mut();
            let items = vec![ContextMenuItem {
                id: Some(WidgetId::new("view")),
                label: quadraui::types::StyledText::plain("View"),
                submenu: Some(vec![ContextMenuItem {
                    id: Some(WidgetId::new("toggle_sidebar")),
                    label: quadraui::types::StyledText::plain("Toggle Sidebar"),
                    ..Default::default()
                }]),
                ..Default::default()
            }];
            app.engine
                .open_context_menu(items, Point::new(1.0, 1.0), backend);
            assert!(app.engine.context_menu_open());
        }
        driver.render();

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Right),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        assert!(
            driver.app().engine.context_menu_open(),
            "opening the submenu must not close the popup"
        );
        assert!(
            driver.app().sink.borrow().dispatched.is_empty(),
            "pure navigation dispatches nothing to the app's own handler"
        );

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        assert!(!driver.app().engine.context_menu_open());
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert_eq!(
            dispatched.len(),
            1,
            "expected one activation: {:?}",
            dispatched
        );
        assert_eq!(
            dispatched[0].0,
            UiEvent::ContextMenuItemActivated(WidgetId::new("toggle_sidebar")),
            "activation must report the nested leaf's own id: {:?}",
            dispatched[0]
        );
    }

    /// The overlay stack orders by [`Overlay::rank`], not by push order:
    /// completions outranks a rich-text popup, so opening completions
    /// FIRST and the popup SECOND must still leave completions on top —
    /// it, not the popup opened more recently, claims the next Escape.
    #[test]
    fn overlay_stack_orders_by_fixed_rank_not_push_order() {
        use quadraui::{Completions, RichTextPopup};

        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);

        let (_backend, app) = driver.parts_mut();
        app.engine.show_completions(
            Completions {
                id: WidgetId::new("completions"),
                ..Default::default()
            },
            Point::new(2.0, 2.0),
        );
        app.engine.show_rich_text_popup(
            RichTextPopup {
                id: WidgetId::new("popup"),
                lines: vec![],
                line_text: vec![],
                line_scales: vec![],
                scroll_top: 0,
                max_visible_rows: 10,
                has_focus: false,
                selection: None,
                links: vec![],
                focused_link: None,
                placement: Default::default(),
                padding: 1.0,
                fg: None,
                bg: None,
            },
            Point::new(2.0, 2.0),
        );

        driver.render();
        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let (_backend, app) = driver.parts_mut();
        assert!(
            !app.engine.dismiss_completions(),
            "Escape must already have closed completions — the higher-\
             ranked overlay, even though it was opened before the popup"
        );
        assert!(
            app.engine.dismiss_rich_text_popup(),
            "the lower-ranked rich-text popup must still be open — Escape \
             only ever closes the topmost overlay"
        );
    }

    /// Dismissing one overlay by id, through the shared stack, leaves
    /// every other entry untouched — another toast with a different id,
    /// and a different overlay kind (`Dialog`) entirely.
    #[test]
    fn dismiss_overlay_by_id_removes_only_the_matching_entry() {
        use quadraui::backend::MessageDialogOptions;

        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);

        let new_toast = |id: &str, title: &str| quadraui::Toast {
            id: WidgetId::new(id),
            title: title.to_string(),
            body: String::new(),
            severity: quadraui::ToastSeverity::Info,
            actions: Vec::new(),
            accent: None,
        };

        {
            let (backend, app) = driver.parts_mut();
            app.engine.open_message_dialog(
                MessageDialogOptions {
                    title: "Heads up".to_string(),
                    body: "Something happened.".to_string(),
                    buttons: Vec::new(),
                    severity: None,
                },
                backend,
            );
            app.engine
                .show_toast(new_toast("a", "A"), quadraui::ToastCorner::BottomRight);
            app.engine
                .show_toast(new_toast("b", "B"), quadraui::ToastCorner::BottomRight);
        }

        let (_backend, app) = driver.parts_mut();
        assert!(
            app.engine.dismiss_toast(WidgetId::new("a")),
            "toast \"a\" was showing"
        );
        assert!(
            !app.engine.dismiss_toast(WidgetId::new("a")),
            "toast \"a\" is already gone"
        );
        assert!(
            app.engine.dialog_open(),
            "the dialog, a different overlay kind sharing the same stack, \
             must be untouched by removing toast \"a\""
        );
        assert!(
            app.engine.dismiss_toast(WidgetId::new("b")),
            "toast \"b\", pushed after \"a\" onto the same shared stack, \
             must be untouched by removing \"a\" by id"
        );
    }

    /// Escape closes only the topmost overlay, one at a time — pressing
    /// it a second time then closes the one underneath, never both at
    /// once.
    #[test]
    fn escape_closes_only_the_top_overlay() {
        use quadraui::RichTextPopup;

        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);

        let (_backend, app) = driver.parts_mut();
        // Rich-text popup opened first (lower `Overlay::rank`), completions
        // opened second (higher rank) — completions ends up on top in
        // push order too, so this also covers the common case alongside
        // `overlay_stack_orders_by_fixed_rank_not_push_order`'s reversed one.
        app.engine.show_rich_text_popup(
            RichTextPopup {
                id: WidgetId::new("popup"),
                lines: vec![],
                line_text: vec![],
                line_scales: vec![],
                scroll_top: 0,
                max_visible_rows: 10,
                has_focus: false,
                selection: None,
                links: vec![],
                focused_link: None,
                placement: Default::default(),
                padding: 1.0,
                fg: None,
                bg: None,
            },
            Point::new(2.0, 2.0),
        );
        app.engine.show_completions(
            quadraui::Completions {
                id: WidgetId::new("completions"),
                ..Default::default()
            },
            Point::new(2.0, 2.0),
        );

        driver.render();

        let escape = || UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        };

        driver.dispatch(escape());
        let (_backend, app) = driver.parts_mut();
        assert!(
            !app.engine.dismiss_completions(),
            "the first Escape must close only the topmost overlay \
             (completions)"
        );
        assert!(
            app.engine.dismiss_rich_text_popup(),
            "the rich-text popup underneath must still be open after just \
             one Escape"
        );

        // Re-open the popup to repeat with a fresh stack and confirm the
        // SECOND Escape (nothing left above it) now closes it too.
        app.engine.show_rich_text_popup(
            RichTextPopup {
                id: WidgetId::new("popup"),
                lines: vec![],
                line_text: vec![],
                line_scales: vec![],
                scroll_top: 0,
                max_visible_rows: 10,
                has_focus: false,
                selection: None,
                links: vec![],
                focused_link: None,
                placement: Default::default(),
                padding: 1.0,
                fg: None,
                bg: None,
            },
            Point::new(2.0, 2.0),
        );
        driver.render();
        driver.dispatch(escape());
        let (_backend, app) = driver.parts_mut();
        assert!(
            !app.engine.dismiss_rich_text_popup(),
            "with nothing left above it, Escape must close the popup too"
        );
    }

    /// A minimal [`AppLogic`] painting a single `DataTable`, for the
    /// header-click/divider-drag/keyboard-nav coverage below — mirrors
    /// [`MinimalLogic`], swapping the button band for a data-table one.
    struct DataTableLogic {
        data_table: DataTable,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for DataTableLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let extra = HashMap::new();
            draw_bands(
                backend,
                RenderRefs {
                    button: None,
                    text_display: None,
                    list: None,
                    tree: None,
                    data_table: Some(&self.data_table),
                    tab_bar: None,
                    extra: &extra,
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
                &|id| self.engine.chrome_interaction(id),
            );
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Builds a two-fixed-column, three-row `DataTable` and drives it
    /// through a [`TuiDriver`], for the header-click/divider-drag/
    /// keyboard-nav tests below to aim a click or drag at exact geometry
    /// (read back via [`data_table_geometry`]) rather than hardcoding
    /// coordinates.
    fn data_table_driver(selected_idx: Option<usize>) -> TuiDriver<DataTableLogic> {
        use quadraui::{Column, ColumnWidth, DataRow, StyledText};

        let data_table = DataTable {
            id: WidgetId::new("table"),
            columns: vec![
                Column {
                    title: "Name".to_string(),
                    width: ColumnWidth::FixedCells(8.0),
                    align: Default::default(),
                },
                Column {
                    title: "Age".to_string(),
                    width: ColumnWidth::FixedCells(8.0),
                    align: Default::default(),
                },
            ],
            rows: vec![
                DataRow {
                    cells: vec![StyledText::plain("Alice"), StyledText::plain("30")],
                    decoration: Default::default(),
                },
                DataRow {
                    cells: vec![StyledText::plain("Bob"), StyledText::plain("40")],
                    decoration: Default::default(),
                },
                DataRow {
                    cells: vec![StyledText::plain("Cleo"), StyledText::plain("50")],
                    decoration: Default::default(),
                },
            ],
            selected_idx,
            scroll_offset: 0,
            sort: None,
            has_focus: false,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: vec![],
            footer: None,
            bordered: false,
        };
        let logic = DataTableLogic {
            data_table,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        TuiDriver::new(logic, 24, 8)
    }

    /// The painted `DataTable`'s own area/layout, read back from the hit
    /// map the same way [`click_on_a_button_focuses_it_then_dispatches_the_click`]
    /// reads a button's bounds — so a test aims at exact geometry instead
    /// of hardcoding coordinates.
    fn data_table_geometry(
        driver: &TuiDriver<DataTableLogic>,
    ) -> (Rect, quadraui::DataTableLayout) {
        let hits = driver.app().engine.hit_map();
        hits.iter()
            .find_map(|hit| match hit {
                crate::hit::PaintedHit::DataTable { area, layout, .. } => {
                    Some((*area, layout.clone()))
                }
                _ => None,
            })
            .expect("data table should have painted a hit")
    }

    /// Clicking a column header must emit `DataTableEvent::HeaderClicked`
    /// (the sort request a Python app toggles into ascending/descending),
    /// never a raw `MouseDown` with a `data_table_header` hit dict left for
    /// the handler to re-interpret itself.
    #[test]
    fn clicking_a_data_table_header_emits_header_clicked() {
        use quadraui::DataTableEvent;

        let mut driver = data_table_driver(None);
        let (area, layout) = data_table_geometry(&driver);
        let header_point = Point::new(
            area.x + layout.columns[0].x + layout.columns[0].width / 2.0,
            area.y + layout.header_height / 2.0,
        );

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: header_point,
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::DataTable(id, DataTableEvent::HeaderClicked { col: 0 })
                    if *id == WidgetId::new("table")
            )),
            "expected a HeaderClicked sort request: {:?}",
            dispatched
        );
    }

    /// Pressing down on a header divider and dragging right must emit
    /// `DataTableEvent::ColumnResized` for the column to its left, growing
    /// its width as the pointer moves — the column-resize drag state
    /// mirroring `ActiveDrag`'s split-divider handling.
    #[test]
    fn dragging_a_header_divider_emits_column_resized() {
        use quadraui::DataTableEvent;

        let mut driver = data_table_driver(None);
        let (area, layout) = data_table_geometry(&driver);
        let original_width = layout.columns[0].width;
        let divider_y = area.y + layout.header_height / 2.0;
        let divider_point = Point::new(area.x + layout.columns[0].x + original_width, divider_y);

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: divider_point,
            modifiers: Modifiers::default(),
        });
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(divider_point.x + 5.0, divider_y),
            buttons: quadraui::ButtonMask {
                left: true,
                right: false,
                middle: false,
            },
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        let resized = dispatched.iter().find_map(|(event, _)| match event {
            UiEvent::DataTable(id, DataTableEvent::ColumnResized { col, width })
                if *id == WidgetId::new("table") =>
            {
                Some((*col, *width))
            }
            _ => None,
        });
        let (col, width) = resized
            .unwrap_or_else(|| panic!("expected a ColumnResized event, got: {:?}", dispatched));
        assert_eq!(col, 0, "the divider drags the column to its left");
        assert!(
            width > original_width,
            "dragging right must grow the column: original={original_width}, new={width}"
        );
    }

    /// Down-arrow navigation on a focused `DataTable` must move the row
    /// selection by one (`DataTableEvent::RowSelected`), not fall through
    /// as a widget-less raw `UiEvent::KeyPressed` the handler has no way to
    /// route back to the table that actually has focus.
    #[test]
    fn down_arrow_on_a_focused_data_table_selects_the_next_row() {
        use quadraui::{DataTableEvent, Key, NamedKey};

        let mut driver = data_table_driver(Some(0));
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("table"))),
                "the data table is a tab stop and must accept focus"
            );
        }

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Down),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::DataTable(id, DataTableEvent::RowSelected { idx: 1 })
                    if *id == WidgetId::new("table")
            )),
            "expected RowSelected {{ idx: 1 }}: {:?}",
            dispatched
        );
    }

    /// Enter on a focused `DataTable` with a selected row must activate
    /// that row (`DataTableEvent::RowActivated`) — the keyboard equivalent
    /// of a pointer double-click, so an app wired to `RowActivated` for
    /// "open" also works from the keyboard, not just a mouse.
    #[test]
    fn enter_on_a_focused_data_table_activates_the_selected_row() {
        use quadraui::{DataTableEvent, Key, NamedKey};

        let mut driver = data_table_driver(Some(0));
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("table"))),
                "the data table is a tab stop and must accept focus"
            );
        }

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::DataTable(id, DataTableEvent::RowActivated { idx: 0 })
                    if *id == WidgetId::new("table")
            )),
            "expected RowActivated {{ idx: 0 }}: {:?}",
            dispatched
        );
    }

    /// Enter on a focused `DataTable` with nothing selected must be a
    /// no-op — there is no row to activate, so it must not fall through
    /// to a raw `UiEvent::KeyPressed` the handler has no table to route
    /// it back to, and must not activate row 0 by default the way
    /// Up/Down's `RowSelected` navigation does.
    #[test]
    fn enter_on_a_focused_data_table_with_no_selection_is_a_no_op() {
        use quadraui::{DataTableEvent, Key, NamedKey};

        let mut driver = data_table_driver(None);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("table"))),
                "the data table is a tab stop and must accept focus"
            );
        }

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            !dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::DataTable(_, DataTableEvent::RowActivated { .. })
            )),
            "no selection means no row to activate: {:?}",
            dispatched
        );
    }

    /// A single click on a `DataTable` row must emit
    /// `DataTableEvent::RowSelected`, not `RowActivated` — the "select"
    /// event, so a row body click never opens anything on its own.
    #[test]
    fn single_click_on_a_data_table_row_emits_row_selected() {
        use quadraui::DataTableEvent;

        let mut driver = data_table_driver(None);
        driver.render();
        let (area, layout) = data_table_geometry(&driver);
        let row_point = Point::new(area.x + 2.0, area.y + layout.header_height + 0.5);

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: row_point,
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::DataTable(id, DataTableEvent::RowSelected { idx: 0 })
                    if *id == WidgetId::new("table")
            )),
            "expected RowSelected {{ idx: 0 }}: {:?}",
            dispatched
        );
        assert!(
            !dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::DataTable(_, DataTableEvent::RowActivated { .. })
            )),
            "a single click must not emit RowActivated: {:?}",
            dispatched
        );
    }

    /// A double-click on a `DataTable` row must emit the distinct
    /// `DataTableEvent::RowDoubleClicked`, not `RowSelected`/`RowActivated`
    /// — the "open" event a file-manager-style app should listen for
    /// instead of opening on every single click.
    #[test]
    fn double_click_on_a_data_table_row_emits_row_double_clicked() {
        use quadraui::DataTableEvent;

        let mut driver = data_table_driver(None);
        driver.render();
        let (area, layout) = data_table_geometry(&driver);
        let row_point = Point::new(area.x + 2.0, area.y + layout.header_height + 0.5);

        driver.dispatch(UiEvent::DoubleClick {
            widget: None,
            position: row_point,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::DataTable(id, DataTableEvent::RowDoubleClicked { idx: 0 })
                    if *id == WidgetId::new("table")
            )),
            "expected RowDoubleClicked {{ idx: 0 }}: {:?}",
            dispatched
        );
    }

    /// A minimal [`AppLogic`] painting a single `Tree`, for the
    /// widget-state cursor/scroll coverage below — mirrors
    /// [`DataTableLogic`], swapping the data-table band for a tree one.
    /// `tree` is the raw descriptor an app would hand a binding (never
    /// mutated by navigation); `render` resolves its cursor through
    /// [`resolve_tree_cursor`] before painting, exactly as a real
    /// binding's own `render` is expected to.
    struct TreeLogic {
        tree: TreeView,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for TreeLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let line_height = backend.line_height();
            // Mirrors `draw_bands`'s own tree-band allocation (4 lines) —
            // the only slot painted here, so it always starts at `y = 0.0`.
            let tree_rect = Rect::new(
                0.0,
                0.0,
                viewport.width,
                (4.0 * line_height).min(viewport.height),
            );
            let viewport_rows = tree_viewport_rows(backend, tree_rect, &self.tree);
            let resolved =
                resolve_tree_cursor(self.engine.widget_state(), &self.tree, viewport_rows);

            let mut hits = Vec::new();
            let extra = HashMap::new();
            draw_bands(
                backend,
                RenderRefs {
                    button: None,
                    text_display: None,
                    list: None,
                    tree: Some(&resolved),
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
                &|id| self.engine.chrome_interaction(id),
            );
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Builds a `row_count`-row `Tree` with no selection/scroll set
    /// (fully uncontrolled when `selected_path`/`scroll_offset` are left
    /// at `None`/`0`) and drives it through a [`TuiDriver`] — mirrors
    /// [`data_table_driver`]. `scroll_offset` lets a caller build a
    /// controlled-scroll tree instead.
    fn tree_driver(
        row_count: usize,
        selected_path: Option<quadraui::TreePath>,
        scroll_offset: usize,
    ) -> TuiDriver<TreeLogic> {
        use quadraui::{StyledText, TreeRow};

        let rows: Vec<TreeRow> = (0..row_count)
            .map(|i| TreeRow {
                path: vec![i as u16],
                indent: 0,
                icon: None,
                text: StyledText::plain(format!("row {i}")),
                badge: None,
                is_expanded: None,
                decoration: Default::default(),
                edit: None,
            })
            .collect();

        let tree = TreeView {
            id: WidgetId::new("tree"),
            rows,
            selection_mode: quadraui::SelectionMode::Single,
            selected_path,
            scroll_offset,
            style: Default::default(),
            has_focus: false,
        };
        let logic = TreeLogic {
            tree,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        TuiDriver::new(logic, 24, 8)
    }

    /// A 500-row `Tree` left fully uncontrolled (no `selected_path`, no
    /// `scroll_offset`) must still move its cursor and keep it scrolled
    /// into view purely from arrow-key presses — the cursor and its
    /// scroll both live in the widget-state store, not the app, per this
    /// module's own doc.
    #[test]
    fn uncontrolled_tree_moves_the_cursor_and_scrolls_it_into_view_on_down() {
        let mut driver = tree_driver(500, None, 0);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("tree"))),
                "the tree is a tab stop and must accept focus"
            );
        }

        for _ in 0..40 {
            driver.dispatch(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                modifiers: Modifiers::default(),
                repeat: false,
            });
        }

        // 40 Downs starting from no selection land on row index 39 (the
        // first Down selects row 0, each of the other 39 moves by one).
        // Checked against the viewport capacity the same hit records
        // (not the literal screen, which a focused tab stop's own
        // separately-painted focus ring can cosmetically overdraw at this
        // tiny band's edges — orthogonal to whether the cursor's row is
        // within the scrolled window this module is responsible for).
        let hits = driver.app().engine.hit_map();
        let tree_hit = hits.iter().find_map(|hit| match hit {
            crate::hit::PaintedHit::Tree {
                id,
                scroll_offset,
                viewport_rows,
                ..
            } if *id == WidgetId::new("tree") => Some((*scroll_offset, *viewport_rows)),
            _ => None,
        });
        let (scroll_offset, viewport_rows) =
            tree_hit.expect("the tree must still be painted after 40 Down presses");
        assert!(
            scroll_offset > 0,
            "500 rows in a {viewport_rows}-row tree viewport must have scrolled: {scroll_offset}"
        );
        assert!(
            (scroll_offset..scroll_offset + viewport_rows).contains(&39),
            "row 39 (the moved cursor) must be within the scrolled window [{scroll_offset}, {})",
            scroll_offset + viewport_rows
        );
    }

    /// A `Tree` whose descriptor pins a non-zero `scroll_offset` keeps
    /// that exact value every frame, even while arrow keys move its
    /// (uncontrolled) selection far enough that an uncontrolled scroll
    /// would have followed it — controlled mode wins outright, per
    /// [`resolve_tree_cursor`]'s own doc.
    #[test]
    fn controlled_tree_scroll_offset_is_pinned_and_not_moved_by_navigation_keys() {
        let mut driver = tree_driver(500, None, 50);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("tree"))),
                "the tree is a tab stop and must accept focus"
            );
        }

        for _ in 0..10 {
            driver.dispatch(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                modifiers: Modifiers::default(),
                repeat: false,
            });
        }

        let hits = driver.app().engine.hit_map();
        let scroll_offset = hits.iter().find_map(|hit| match hit {
            crate::hit::PaintedHit::Tree {
                id, scroll_offset, ..
            } if *id == WidgetId::new("tree") => Some(*scroll_offset),
            _ => None,
        });
        assert_eq!(
            scroll_offset,
            Some(50),
            "a controlled scroll_offset must stay pinned at the descriptor's own value: {:?}",
            scroll_offset
        );
    }

    /// A minimal [`AppLogic`] painting a single `ListView` through
    /// [`resolve_list_cursor`] — the `List` counterpart of [`TreeLogic`],
    /// for the widget-state cursor/scroll coverage below.
    struct ListCursorLogic {
        list: ListView,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for ListCursorLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let line_height = backend.line_height();
            // Mirrors `draw_bands`'s own list-band allocation (4 lines).
            let list_rect = Rect::new(
                0.0,
                0.0,
                viewport.width,
                (4.0 * line_height).min(viewport.height),
            );
            let viewport_rows = list_viewport_rows(backend, list_rect, &self.list);
            let resolved =
                resolve_list_cursor(self.engine.widget_state(), &self.list, viewport_rows);

            let mut hits = Vec::new();
            let extra = HashMap::new();
            draw_bands(
                backend,
                RenderRefs {
                    button: None,
                    text_display: None,
                    list: Some(&resolved),
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
                &|id| self.engine.chrome_interaction(id),
            );
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Builds a `row_count`-row `ListView` with the given
    /// `selected_idx`/`scroll_offset` and drives it through a
    /// [`TuiDriver`] — mirrors [`tree_driver`].
    fn list_cursor_driver(
        row_count: usize,
        selected_idx: usize,
        scroll_offset: usize,
    ) -> TuiDriver<ListCursorLogic> {
        use quadraui::{ListItem, StyledText};

        let list = ListView {
            id: WidgetId::new("list-cursor"),
            title: None,
            items: (0..row_count)
                .map(|i| ListItem {
                    text: StyledText::plain(format!("row {i}")),
                    icon: None,
                    detail: None,
                    decoration: Default::default(),
                })
                .collect(),
            selected_idx,
            scroll_offset,
            has_focus: false,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        };
        let logic = ListCursorLogic {
            list,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        TuiDriver::new(logic, 24, 8)
    }

    /// A 500-row `List` left fully uncontrolled (`selected_idx: 0`,
    /// `scroll_offset: 0`) must still move its cursor and keep it
    /// scrolled into view purely from arrow-key presses — the `List`
    /// counterpart of
    /// [`uncontrolled_tree_moves_the_cursor_and_scrolls_it_into_view_on_down`].
    #[test]
    fn uncontrolled_list_moves_the_cursor_and_scrolls_it_into_view_on_down() {
        let mut driver = list_cursor_driver(500, 0, 0);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("list-cursor"))),
                "the list is a tab stop and must accept focus"
            );
        }

        for _ in 0..40 {
            driver.dispatch(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                modifiers: Modifiers::default(),
                repeat: false,
            });
        }

        let hits = driver.app().engine.hit_map();
        let list_hit = hits.iter().find_map(|hit| match hit {
            crate::hit::PaintedHit::List {
                id,
                selected_idx,
                scroll_offset,
                viewport_rows,
                ..
            } if *id == WidgetId::new("list-cursor") => {
                Some((*selected_idx, *scroll_offset, *viewport_rows))
            }
            _ => None,
        });
        let (selected_idx, scroll_offset, viewport_rows) =
            list_hit.expect("the list must still be painted after 40 Down presses");
        assert_eq!(
            selected_idx, 39,
            "40 Downs starting from no selection must land exactly on row 39"
        );
        assert!(
            (scroll_offset..scroll_offset + viewport_rows).contains(&39),
            "row 39 (the moved cursor) must be within the scrolled window [{scroll_offset}, {})",
            scroll_offset + viewport_rows
        );
    }

    /// A `List` whose descriptor pins a non-zero `scroll_offset` keeps
    /// that exact value every frame, even while arrow keys move its
    /// (uncontrolled) selection — the `List` counterpart of
    /// [`controlled_tree_scroll_offset_is_pinned_and_not_moved_by_navigation_keys`].
    #[test]
    fn controlled_list_scroll_offset_is_pinned_and_not_moved_by_navigation_keys() {
        let mut driver = list_cursor_driver(500, 0, 50);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("list-cursor"))),
                "the list is a tab stop and must accept focus"
            );
        }

        for _ in 0..10 {
            driver.dispatch(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                modifiers: Modifiers::default(),
                repeat: false,
            });
        }

        let hits = driver.app().engine.hit_map();
        let scroll_offset = hits.iter().find_map(|hit| match hit {
            crate::hit::PaintedHit::List {
                id, scroll_offset, ..
            } if *id == WidgetId::new("list-cursor") => Some(*scroll_offset),
            _ => None,
        });
        assert_eq!(
            scroll_offset,
            Some(50),
            "a controlled scroll_offset must stay pinned at the descriptor's own value: {:?}",
            scroll_offset
        );
    }

    /// Moving a `List`'s stored cursor is invisible to the app's own event
    /// handler: a navigation key still arrives as the focus-scoped raw
    /// `ListViewEvent::KeyPressed`, never as a synthesised
    /// `SelectionChanged`. That is what an app tracking its own
    /// `ListView::selected_idx` (the controlled shape) needs in order to
    /// move that selection itself, so the widget-state cursor must not take
    /// the event away from it.
    #[test]
    fn list_navigation_keys_reach_the_app_as_the_raw_key_pressed_event() {
        use quadraui::ListViewEvent;

        let mut driver = list_cursor_driver(500, 0, 0);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("list-cursor"))),
                "the list is a tab stop and must accept focus"
            );
        }

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Down),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let dispatched = driver.app().sink.borrow().dispatched.clone();
        let list_events: Vec<_> = dispatched
            .iter()
            .filter_map(|(event, _)| match event {
                UiEvent::List(id, inner) if *id == WidgetId::new("list-cursor") => Some(inner),
                _ => None,
            })
            .collect();
        assert_eq!(
            list_events.len(),
            1,
            "exactly one list event must reach the app: {dispatched:?}"
        );
        assert!(
            matches!(list_events[0], ListViewEvent::KeyPressed { .. }),
            "Down must stay a raw focus-scoped KeyPressed: {:?}",
            list_events[0]
        );
    }

    /// A minimal [`AppLogic`] painting a single `DataTable` through
    /// [`resolve_data_table_cursor`] — the `DataTable` counterpart of
    /// [`TreeLogic`]/[`ListCursorLogic`], for the widget-state
    /// cursor/scroll coverage below.
    struct DataTableCursorLogic {
        table: DataTable,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for DataTableCursorLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let line_height = backend.line_height();
            // Mirrors `draw_bands`'s own data-table-band allocation (4 lines).
            let table_rect = Rect::new(
                0.0,
                0.0,
                viewport.width,
                (4.0 * line_height).min(viewport.height),
            );
            let viewport_rows = backend
                .data_table_layout(table_rect, &self.table)
                .visible_rows;
            let resolved =
                resolve_data_table_cursor(self.engine.widget_state(), &self.table, viewport_rows);

            let mut hits = Vec::new();
            let extra = HashMap::new();
            draw_bands(
                backend,
                RenderRefs {
                    button: None,
                    text_display: None,
                    list: None,
                    tree: None,
                    data_table: Some(&resolved),
                    tab_bar: None,
                    extra: &extra,
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
                &|id| self.engine.chrome_interaction(id),
            );
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Builds a `row_count`-row `DataTable` with the given
    /// `selected_idx`/`scroll_offset` and drives it through a
    /// [`TuiDriver`] — mirrors [`tree_driver`]/[`list_cursor_driver`].
    fn data_table_cursor_driver(
        row_count: usize,
        selected_idx: Option<usize>,
        scroll_offset: usize,
    ) -> TuiDriver<DataTableCursorLogic> {
        use quadraui::{Column, DataRow, StyledText};

        let table = DataTable {
            id: WidgetId::new("table-cursor"),
            columns: vec![Column {
                title: "Col".to_string(),
                width: Default::default(),
                align: Default::default(),
            }],
            rows: (0..row_count)
                .map(|i| DataRow {
                    cells: vec![StyledText::plain(format!("row {i}"))],
                    decoration: Default::default(),
                })
                .collect(),
            selected_idx,
            scroll_offset,
            sort: None,
            has_focus: false,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: Vec::new(),
            footer: None,
            bordered: false,
        };
        let logic = DataTableCursorLogic {
            table,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        TuiDriver::new(logic, 24, 8)
    }

    /// A 500-row `DataTable` left fully uncontrolled (`selected_idx: None`,
    /// `scroll_offset: 0`) must still move its cursor and keep it
    /// scrolled into view purely from arrow-key presses — the `DataTable`
    /// counterpart of
    /// [`uncontrolled_tree_moves_the_cursor_and_scrolls_it_into_view_on_down`].
    #[test]
    fn uncontrolled_data_table_moves_the_cursor_and_scrolls_it_into_view_on_down() {
        let mut driver = data_table_cursor_driver(500, None, 0);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("table-cursor"))),
                "the data table is a tab stop and must accept focus"
            );
        }

        for _ in 0..40 {
            driver.dispatch(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                modifiers: Modifiers::default(),
                repeat: false,
            });
        }

        let hits = driver.app().engine.hit_map();
        let table_hit = hits.iter().find_map(|hit| match hit {
            crate::hit::PaintedHit::DataTable {
                id,
                selected_idx,
                scroll_offset,
                viewport_rows,
                ..
            } if *id == WidgetId::new("table-cursor") => {
                Some((*selected_idx, *scroll_offset, *viewport_rows))
            }
            _ => None,
        });
        let (selected_idx, scroll_offset, viewport_rows) =
            table_hit.expect("the table must still be painted after 40 Down presses");
        assert_eq!(
            selected_idx,
            Some(39),
            "40 Downs starting from no selection must land exactly on row 39"
        );
        assert!(
            (scroll_offset..scroll_offset + viewport_rows).contains(&39),
            "row 39 (the moved cursor) must be within the scrolled window [{scroll_offset}, {})",
            scroll_offset + viewport_rows
        );
    }

    /// A `DataTable` whose descriptor pins a non-zero `scroll_offset`
    /// keeps that exact value every frame, even while arrow keys move its
    /// (uncontrolled) selection — the `DataTable` counterpart of
    /// [`controlled_tree_scroll_offset_is_pinned_and_not_moved_by_navigation_keys`].
    #[test]
    fn controlled_data_table_scroll_offset_is_pinned_and_not_moved_by_navigation_keys() {
        let mut driver = data_table_cursor_driver(500, None, 50);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("table-cursor"))),
                "the data table is a tab stop and must accept focus"
            );
        }

        for _ in 0..10 {
            driver.dispatch(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                modifiers: Modifiers::default(),
                repeat: false,
            });
        }

        let hits = driver.app().engine.hit_map();
        let scroll_offset = hits.iter().find_map(|hit| match hit {
            crate::hit::PaintedHit::DataTable {
                id, scroll_offset, ..
            } if *id == WidgetId::new("table-cursor") => Some(*scroll_offset),
            _ => None,
        });
        assert_eq!(
            scroll_offset,
            Some(50),
            "a controlled scroll_offset must stay pinned at the descriptor's own value: {:?}",
            scroll_offset
        );
    }

    /// A minimal [`AppLogic`] painting a single `ListView`, for the
    /// Enter-activates-the-selection coverage below — mirrors
    /// [`DataTableLogic`], swapping the data-table band for a list one.
    struct ListLogic {
        list: quadraui::ListView,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for ListLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let extra = HashMap::new();
            draw_bands(
                backend,
                RenderRefs {
                    button: None,
                    text_display: None,
                    list: Some(&self.list),
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
                &|id| self.engine.chrome_interaction(id),
            );
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Builds a three-row `ListView` (empty `items` when `item_count` is
    /// `0`, for the no-selection no-op case) and drives it through a
    /// [`TuiDriver`] — mirrors [`data_table_driver`].
    fn list_driver(item_count: usize, selected_idx: usize) -> TuiDriver<ListLogic> {
        use quadraui::{ListItem, StyledText};

        let list = quadraui::ListView {
            id: WidgetId::new("list"),
            title: None,
            items: (0..item_count)
                .map(|i| ListItem {
                    text: StyledText::plain(format!("item {i}")),
                    icon: None,
                    detail: None,
                    decoration: Default::default(),
                })
                .collect(),
            selected_idx,
            scroll_offset: 0,
            has_focus: false,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        };
        let logic = ListLogic {
            list,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        TuiDriver::new(logic, 24, 8)
    }

    /// Enter on a focused `List` with at least one row must activate the
    /// current selection (`ListViewEvent::ItemActivated`) — the keyboard
    /// equivalent of a pointer double-click, so an app wired to
    /// `ItemActivated` for "open" also works from the keyboard, not just a
    /// mouse.
    #[test]
    fn enter_on_a_focused_list_activates_the_selected_row() {
        use quadraui::{Key, ListViewEvent, NamedKey};

        let mut driver = list_driver(3, 1);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("list"))),
                "the list is a tab stop and must accept focus"
            );
        }

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::List(id, ListViewEvent::ItemActivated { idx: 1 })
                    if *id == WidgetId::new("list")
            )),
            "expected ItemActivated {{ idx: 1 }}: {:?}",
            dispatched
        );
    }

    /// Enter on a focused `List` with no items must be a no-op — there is
    /// no row to activate, so it must not fall through to a raw
    /// `UiEvent::KeyPressed` the handler has no list to route it back to.
    #[test]
    fn enter_on_a_focused_list_with_no_items_is_a_no_op() {
        use quadraui::{Key, ListViewEvent, NamedKey};

        let mut driver = list_driver(0, 0);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("list"))),
                "the list is a tab stop and must accept focus"
            );
        }

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            !dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::List(_, ListViewEvent::ItemActivated { .. })
            )),
            "no items means no row to activate: {:?}",
            dispatched
        );
    }

    /// A minimal [`AppLogic`] painting a single `TextDisplay`, for the
    /// titled-display hit-test/wheel-scroll coverage below — mirrors
    /// [`DataTableLogic`], swapping the data-table band for a text-display
    /// one.
    struct TextDisplayLogic {
        text_display: quadraui::TextDisplay,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for TextDisplayLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let extra = HashMap::new();
            draw_bands(
                backend,
                RenderRefs {
                    button: None,
                    text_display: Some(&self.text_display),
                    list: None,
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
                &|id| self.engine.chrome_interaction(id),
            );
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Builds a titled, 5-line `TextDisplay` (more lines than the fixed
    /// 3-line band can show once the title row eats into it) and drives it
    /// through a [`TuiDriver`] — the regression shape for the body-offset
    /// bug: every backend's `text_display_layout` lays the body out from
    /// `0.0` and leaves the title row to be painted separately above it
    /// (see [`crate::hit::PaintedHit::TextDisplay`]'s own doc), so a hit
    /// test that only subtracts `area`'s origin (not also the title row's
    /// height) misreads which body line a click/scroll landed on.
    fn titled_text_display_driver() -> TuiDriver<TextDisplayLogic> {
        use quadraui::{StyledSpan, StyledText, TextDisplay, TextDisplayLine};

        let text_display = TextDisplay {
            id: WidgetId::new("log"),
            lines: (0..5)
                .map(|i| TextDisplayLine {
                    spans: vec![StyledSpan::plain(format!("line {i}"))],
                    decoration: Default::default(),
                    timestamp: None,
                })
                .collect(),
            scroll_offset: 0,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: Some(StyledText::plain("Logs")),
            show_scrollbar: false,
        };
        let logic = TextDisplayLogic {
            text_display,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        TuiDriver::new(logic, 40, 10)
    }

    /// The painted `TextDisplay`'s own area/body-offset/viewport-line-count,
    /// read back the same way [`data_table_geometry`] reads a table's — so
    /// a test aims at exact geometry instead of hardcoding how many rows
    /// the fixed-slot band leaves for the body.
    fn text_display_geometry(driver: &TuiDriver<TextDisplayLogic>) -> (Rect, f32, usize, usize) {
        let hits = driver.app().engine.hit_map();
        hits.iter()
            .find_map(|hit| match hit {
                crate::hit::PaintedHit::TextDisplay {
                    area,
                    body_offset,
                    total_lines,
                    viewport_lines,
                    ..
                } => Some((*area, *body_offset, *total_lines, *viewport_lines)),
                _ => None,
            })
            .expect("text display should have painted a hit")
    }

    /// A click on the title row of a titled `TextDisplay` must NOT resolve
    /// to `TextDisplayLine { line: 0 }` — the body-local layout's line 0
    /// sits one row below the title, so a hit at the title's own screen
    /// position has to miss the layout entirely (translated to a negative
    /// local y) rather than silently reading as the first body line.
    #[test]
    fn click_on_titled_text_display_title_row_does_not_resolve_as_body_line_zero() {
        let mut driver = titled_text_display_driver();
        let (area, body_offset, _, _) = text_display_geometry(&driver);
        assert!(
            body_offset > 0.0,
            "a titled display must report a body offset"
        );
        let title_point = Point::new(area.x + 1.0, area.y);

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: title_point,
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        let hit_body_line_zero = dispatched
            .iter()
            .any(|(_, hit)| matches!(hit, Some(HitDetail::TextDisplayLine { line: 0 })));
        assert!(
            !hit_body_line_zero,
            "a click on the title row must not be misread as body line 0: {:?}",
            dispatched
        );
    }

    /// A wheel notch over the bottom-most visible body row of a titled
    /// `TextDisplay` must emit `TextDisplayEvent::Scrolled`. Before the
    /// body-offset fix, that row's screen position translated to a local y
    /// past every `hit_regions` entry (the layout only covers
    /// `viewport_lines` rows starting at `0.0`), so the hit test missed
    /// entirely and the wheel event fell through untranslated.
    #[test]
    fn wheel_scroll_over_titled_text_display_bottom_row_emits_scrolled() {
        use quadraui::TextDisplayEvent;

        let mut driver = titled_text_display_driver();
        let (area, body_offset, total_lines, viewport_lines) = text_display_geometry(&driver);
        assert!(
            total_lines > viewport_lines,
            "the display must have more lines than fit, or there's nothing to scroll"
        );
        let bottom_row_point = Point::new(
            area.x + 1.0,
            area.y + body_offset + (viewport_lines as f32 - 0.5),
        );

        driver.dispatch(UiEvent::Scroll {
            widget: None,
            delta: quadraui::ScrollDelta::new(0.0, -1.0),
            position: bottom_row_point,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::TextDisplay(id, TextDisplayEvent::Scrolled { new_offset: 1 })
                    if *id == WidgetId::new("log")
            )),
            "expected TextDisplay(Scrolled {{ new_offset: 1 }}): {:?}",
            dispatched
        );
    }

    /// A minimal [`AppLogic`] painting a single `quadraui::SplitTree` node
    /// (via [`paint_layout`], not [`draw_bands`] — `SplitTree` is a
    /// `LayoutNode` shape, not one of the six fixed slots), for the
    /// divider-drag coverage below.
    struct SplitTreeLogic {
        tree: quadraui::SplitTree,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for SplitTreeLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let node = LayoutNode::SplitTree(LayoutSplitTree {
                id: "tree".to_string(),
                tree: self.tree.clone(),
                leaves: vec![
                    LayoutNode::Leaf(LayoutLeaf {
                        id: "left".to_string(),
                        descriptor: LeafDescriptor::Button(Button::new(
                            WidgetId::new("left-btn"),
                            "L",
                        )),
                        scroll_px: None,
                    }),
                    LayoutNode::Leaf(LayoutLeaf {
                        id: "right".to_string(),
                        descriptor: LeafDescriptor::Button(Button::new(
                            WidgetId::new("right-btn"),
                            "R",
                        )),
                        scroll_px: None,
                    }),
                ],
            });
            paint_layout(backend, area, &node, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Dragging a `SplitTree` divider must turn the following `MouseMoved`
    /// into `UiEvent::SplitDividerDragged` with the resolved ratio — the
    /// N-way generalisation of [`dragging_a_header_divider_emits_column_resized`]'s
    /// coverage, exercising [`crate::drag::ActiveDrag::SplitTree`] instead
    /// of the two-pane [`crate::drag::ActiveDrag::Split`] case.
    #[test]
    fn dragging_a_split_tree_divider_changes_the_ratio() {
        use quadraui::SplitDirection;

        // Ratio 0.6 (not the more obvious 0.5) so the resolved divider
        // `position` has a fractional part under `0.5` — `first_w` then
        // rounds DOWN to the Button leaf's own painted width, so the leaf
        // doesn't also claim the divider's painted cell the way it would
        // at a dead-even tie (`width * 0.5` on an even width rounds UP,
        // per `f32::round`'s ties-away-from-zero rule, which is a
        // pre-existing, separate leaf/divider hit-box overlap this test
        // isn't about).
        let tree = quadraui::SplitTree::split(
            SplitDirection::Horizontal,
            0.6,
            quadraui::SplitTree::leaf(WidgetId::new("left-btn")),
            quadraui::SplitTree::leaf(WidgetId::new("right-btn")),
        );
        let logic = SplitTreeLogic {
            tree,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        // The module's usual 40-cell-wide viewport — deliberately even, so
        // the divider's resolved `position` is fractional and the click
        // below exercises the truncate-to-painted-cell path
        // `hit_test_painted`'s `PaintedHit::SplitTree` arm needs (an odd
        // width like 41 lands the divider on an exact integer and would
        // hide a regression there).
        let mut driver = TuiDriver::new(logic, 40, 10);
        driver.render();

        // Read the divider's own painted geometry back off the hit map
        // (the `data_table_geometry` pattern) instead of hand-deriving the
        // click column, so the test can't drift from what was actually
        // painted.
        let layout = {
            let hits = driver.app().engine.hit_map();
            find_split_tree_layout(&hits, WidgetId::new("tree"))
                .expect("split tree should have painted a hit")
        };
        let divider = &layout.dividers[0];
        assert!(
            divider.position.fract() > 0.0,
            "expected a fractional divider position on an even-width viewport, got {}",
            divider.position
        );
        let divider_cell = divider.cell_position() as f32;

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(divider_cell, 0.0),
            modifiers: Modifiers::default(),
        });
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(divider_cell + 10.0, 0.0),
            buttons: quadraui::ButtonMask {
                left: true,
                right: false,
                middle: false,
            },
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        let dragged = dispatched.iter().find_map(|(event, _)| match event {
            UiEvent::SplitDividerDragged {
                tree,
                split_index,
                new_ratio,
            } if *tree == WidgetId::new("tree") => Some((*split_index, *new_ratio)),
            _ => None,
        });
        let (split_index, new_ratio) = dragged
            .unwrap_or_else(|| panic!("expected a SplitDividerDragged event: {:?}", dispatched));
        assert_eq!(split_index, 0);
        // ratio = (cursor_axis - axis_start) / axis_size, the same
        // `dispatch_mouse_moved` conversion `drag.rs`'s `ActiveDrag::SplitTree::ratio_at`
        // now mirrors: (19 + 10 - 0) / 40 == 0.725.
        let expected_ratio = (divider_cell + 10.0 - divider.axis_start) / divider.axis_size;
        assert!(
            (new_ratio - expected_ratio).abs() < 0.001,
            "expected a ratio around {expected_ratio}, got {new_ratio}"
        );
    }

    /// A minimal [`AppLogic`] painting a single two-pane `quadraui::Split`
    /// node (via [`paint_layout`]'s `LayoutNode::Split` arm), resolving the
    /// ratio through [`resolve_split_ratio`] before every render — the
    /// `Split` counterpart of [`SplitTreeLogic`]. `split` is the raw
    /// descriptor an app would hand a binding (never mutated by a drag),
    /// mirroring [`TreeLogic::tree`].
    struct SplitLogic {
        split: Split,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for SplitLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let resolved = resolve_split_ratio(self.engine.widget_state(), &self.split);
            let mut hits = Vec::new();
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let node = LayoutNode::Split(LayoutSplit {
                split: resolved,
                first: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "left".to_string(),
                    descriptor: LeafDescriptor::Button(Button::new(WidgetId::new("left-btn"), "L")),
                    scroll_px: None,
                })),
                second: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "right".to_string(),
                    descriptor: LeafDescriptor::Button(Button::new(
                        WidgetId::new("right-btn"),
                        "R",
                    )),
                    scroll_px: None,
                })),
                spacing: Spacing::default(),
            });
            paint_layout(backend, area, &node, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Dragging a two-pane `Split`'s divider persists the resolved ratio
    /// into the widget-state store, so a later render with the app's own
    /// `Split::ratio` left at its original, never-updated `0.5` (the
    /// "uncontrolled, no app state" shape this module's own doc describes)
    /// still paints the dragged ratio — this crate's acceptance contract
    /// for the split-ratio half of the widget-state store, exercised
    /// end-to-end through [`InteractionEngine::handle`] rather than only
    /// at the [`resolve_split_ratio`]/[`crate::host::apply_split_divider_drag`]
    /// unit level `crate::host::tests` already covers.
    #[test]
    fn dragging_a_split_divider_persists_the_ratio_into_the_widget_state_store() {
        let split = Split {
            id: WidgetId::new("split"),
            direction: quadraui::SplitDirection::Horizontal,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        };
        let logic = SplitLogic {
            split,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 10);
        driver.render();

        let divider_bounds = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    crate::hit::PaintedHit::Split { layout } => Some(layout.divider_bounds),
                    _ => None,
                })
                .expect("the split should have painted a divider")
        };
        // `divider_bounds.x` itself, not its midpoint: at an exact `0.5`
        // ratio on an even-width viewport the midpoint lands on the exact
        // boundary with the second pane's hit region, which wins the tie
        // (`SplitLayout::hit_test` checks regions in order) and the click
        // lands on a button instead of the divider.
        let divider_point = Point::new(
            divider_bounds.x,
            divider_bounds.y + divider_bounds.height / 2.0,
        );

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: divider_point,
            modifiers: Modifiers::default(),
        });
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(divider_point.x + 15.0, divider_point.y),
            buttons: quadraui::ButtonMask {
                left: true,
                right: false,
                middle: false,
            },
        });

        let dragged_ratio = {
            let dispatched = &driver.app().sink.borrow().dispatched;
            dispatched
                .iter()
                .find_map(|(event, _)| match event {
                    UiEvent::SplitDividerDragged {
                        tree, new_ratio, ..
                    } if *tree == WidgetId::new("split") => Some(*new_ratio),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("expected a SplitDividerDragged event: {:?}", dispatched))
        };
        assert_ne!(
            dragged_ratio, 0.5,
            "the drag must have actually moved the ratio away from its starting 0.5"
        );

        // The app's own `Split` descriptor is never touched by the drag —
        // this render still hands `paint_layout` the exact same `ratio:
        // 0.5` it always has, the "uncontrolled, no app state" shape this
        // test is about.
        driver.render();
        let resolved_ratio = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    crate::hit::PaintedHit::Split { layout } => Some(layout.resolved_ratio),
                    _ => None,
                })
                .expect("the split should still have painted a divider")
        };
        assert!(
            (resolved_ratio - dragged_ratio).abs() < 0.001,
            "the dragged ratio must survive a render whose descriptor still says 0.5: got {resolved_ratio}, expected {dragged_ratio}"
        );
    }

    /// Clicking a tab (`TabBarEvent::TabActivated`, dispatched the same
    /// already-semantic shape a real click on a tab body resolves to)
    /// persists the active index into the widget-state store, so a later
    /// render with the app's own `TabBar` descriptor left exactly as it
    /// started (no tab marked active — "uncontrolled, no app state") still
    /// shows the clicked tab active. Exercises
    /// [`InteractionEngine::handle`]'s own persistence hook directly,
    /// rather than only [`resolve_tab_bar_active`]/
    /// [`crate::host::apply_tab_activated`] at the `crate::host::tests`
    /// unit level.
    #[test]
    fn clicking_a_tab_persists_the_active_index_into_the_widget_state_store() {
        let id = WidgetId::new("tabs");
        let mut engine = InteractionEngine::new();
        let mut sink = RecordingSink::default();
        let mut backend = quadraui::tui::TuiBackend::new();

        engine.handle(
            UiEvent::TabBar(id, TabBarEvent::TabActivated { index: 2 }),
            &mut backend,
            &mut sink,
        );

        let tab_bar = quadraui::TabBar {
            id,
            tabs: (0..4)
                .map(|i| quadraui::TabItem {
                    label: format!("tab {i}"),
                    ..Default::default()
                })
                .collect(),
            scroll_offset: 0,
            right_segments: Vec::new(),
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };
        let resolved = resolve_tab_bar_active(engine.widget_state(), &tab_bar);
        assert!(
            resolved.tabs[2].is_active,
            "the clicked tab must stay active even though the descriptor never marks any tab active"
        );
    }

    /// A minimal [`AppLogic`] painting a single `LayoutTabs` tab group (via
    /// [`paint_layout`]'s `LayoutNode::Tabs` arm), resolving it through
    /// [`resolve_layout_tabs_active`] before every render — the `LayoutTabs`
    /// counterpart of [`SplitLogic`]. Rebuilds a fresh [`LayoutTabs`] from
    /// `tab_count` on every render (never mutated by a click), mirroring
    /// [`SplitLogic::split`]'s "raw descriptor an app would hand a binding"
    /// shape; `LayoutNode` isn't `Clone` (see [`crate::host::resolve_layout_tabs_active`]'s
    /// own doc), so there's no stored `LayoutTabs` field to clone from
    /// instead.
    struct TabsLogic {
        tab_count: usize,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl TabsLogic {
        fn build_tabs(&self) -> LayoutTabs {
            let tab_bar = TabBar {
                id: WidgetId::new("tabs"),
                tabs: (0..self.tab_count)
                    .map(|i| quadraui::TabItem {
                        label: format!("Tab {i}"),
                        ..Default::default()
                    })
                    .collect(),
                scroll_offset: 0,
                right_segments: Vec::new(),
                active_accent: None,
                show_tab_close: false,
                compact: false,
            };
            let entries = (0..self.tab_count)
                .map(|i| LayoutTabEntry {
                    id: format!("content-{i}"),
                    content: Box::new(LayoutNode::Leaf(LayoutLeaf {
                        id: format!("content-{i}-btn"),
                        descriptor: LeafDescriptor::Button(Button::new(
                            WidgetId::new(format!("content-{i}-btn")),
                            format!("Tab {i} body"),
                        )),
                        scroll_px: None,
                    })),
                })
                .collect();
            LayoutTabs {
                tab_bar,
                entries,
                selected: 0,
                spacing: Spacing::default(),
                focused_index: None,
            }
        }
    }

    impl AppLogic for TabsLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut tabs = self.build_tabs();
            resolve_layout_tabs_active(self.engine.widget_state(), &mut tabs);
            let mut hits = Vec::new();
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let node = LayoutNode::Tabs(tabs);
            paint_layout(backend, area, &node, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Clicking a tab body inside a `LayoutTabs` tab group — with no app
    /// state at all, the descriptor keeps rebuilding the exact same
    /// `selected: 0` every render — moves BOTH the strip's own active
    /// highlight AND the content pane `paint_layout`'s `LayoutNode::Tabs`
    /// arm paints underneath it. This is the acceptance case
    /// `resolve_tab_bar_active` alone cannot satisfy: a binding that
    /// resolved only the strip would highlight tab 1 while this render
    /// kept painting tab 0's own body underneath it.
    #[test]
    fn clicking_a_tab_in_a_layout_tabs_group_moves_both_the_strip_and_the_content_pane() {
        let logic = TabsLogic {
            tab_count: 3,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 10);
        driver.render();

        // Starts on tab 0's own content, never tab 1's.
        {
            let hits = driver.app().engine.hit_map();
            assert!(hits.iter().any(|hit| matches!(
                hit,
                crate::hit::PaintedHit::Button { id, .. } if *id == WidgetId::new("content-0-btn")
            )));
        }

        let (strip_area, strip_layout) = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    crate::hit::PaintedHit::TabBar { area, layout, .. } => {
                        Some((*area, layout.clone()))
                    }
                    _ => None,
                })
                .expect("the tab strip should have painted")
        };
        let tab1_rect = strip_layout
            .hit_regions
            .iter()
            .find_map(|(rect, hit)| match hit {
                TabBarHit::Tab(1) => Some(*rect),
                _ => None,
            })
            .expect("tab 1 should have its own hit region");
        let click_point = Point::new(
            strip_area.x + tab1_rect.x + tab1_rect.width / 2.0,
            strip_area.y + tab1_rect.y + tab1_rect.height / 2.0,
        );

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: click_point,
            modifiers: Modifiers::default(),
        });
        driver.dispatch(UiEvent::MouseUp {
            widget: None,
            button: MouseButton::Left,
            position: click_point,
        });

        // The app's own `LayoutTabs` is never touched by the click — every
        // render still rebuilds the exact same `selected: 0` it always
        // has, the "uncontrolled, no app state" shape this test is about.
        driver.render();
        let hits = driver.app().engine.hit_map();
        assert!(
            hits.iter().any(|hit| matches!(
                hit,
                crate::hit::PaintedHit::Button { id, .. } if *id == WidgetId::new("content-1-btn")
            )),
            "the content pane must follow the clicked tab, not stay on the descriptor's own tab 0"
        );
        assert!(
            !hits.iter().any(|hit| matches!(
                hit,
                crate::hit::PaintedHit::Button { id, .. } if *id == WidgetId::new("content-0-btn")
            )),
            "tab 0's own content must stop painting once tab 1 is active"
        );
        assert!(
            strip_layout
                .hit_regions
                .iter()
                .any(|(_, hit)| matches!(hit, TabBarHit::Tab(1))),
            "sanity: tab 1 must have been clickable in the first place"
        );
    }

    /// Toggling a tree row's expand state (`TreeEvent::RowToggleExpand`,
    /// dispatched the same already-semantic shape a real click on a
    /// chevron resolves to) persists it into the widget-state store, so a
    /// later render with the app's own `TreeView` descriptor left exactly
    /// as it started ("uncontrolled, no app state") still shows the row
    /// expanded. Exercises [`InteractionEngine::handle`]'s own persistence
    /// hook directly, rather than only [`resolve_tree_expansion`]/
    /// [`crate::host::apply_tree_toggle_expand`] at the `crate::host::tests`
    /// unit level.
    #[test]
    fn toggling_a_tree_row_persists_the_expand_state_into_the_widget_state_store() {
        let id = WidgetId::new("tree");
        let path = vec![0_u16];
        let mut engine = InteractionEngine::new();
        let mut sink = RecordingSink::default();
        let mut backend = quadraui::tui::TuiBackend::new();

        let tree = TreeView {
            id,
            rows: vec![quadraui::TreeRow {
                path: path.clone(),
                indent: 0,
                icon: None,
                text: quadraui::StyledText::plain("row"),
                badge: None,
                is_expanded: Some(false),
                decoration: Default::default(),
                edit: None,
            }],
            selection_mode: quadraui::SelectionMode::Single,
            selected_path: None,
            scroll_offset: 0,
            style: Default::default(),
            has_focus: false,
        };
        // A render always happens before a click can land on a chevron —
        // this seeds `resolve_tree_expansion`'s own baseline from the
        // descriptor's starting `is_expanded: Some(false)`.
        resolve_tree_expansion(engine.widget_state(), &tree);

        engine.handle(
            UiEvent::Tree(id, TreeEvent::RowToggleExpand { path }),
            &mut backend,
            &mut sink,
        );

        let resolved = resolve_tree_expansion(engine.widget_state(), &tree);
        assert_eq!(
            resolved.rows[0].is_expanded,
            Some(true),
            "the toggled row must stay expanded even though the descriptor never changes"
        );
    }

    /// A minimal [`AppLogic`] painting a single `quadraui::Panel` node, for
    /// the title-bar-action-click coverage below.
    struct PanelLogic {
        panel: quadraui::Panel,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for PanelLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let node = LayoutNode::Panel(LayoutPanel {
                panel: self.panel.clone(),
                content: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "body".to_string(),
                    descriptor: LeafDescriptor::Button(Button::new(
                        WidgetId::new("body-btn"),
                        "Body",
                    )),
                    scroll_px: None,
                })),
                spacing: crate::layout::Spacing::default(),
            });
            paint_layout(backend, area, &node, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Clicking a `Panel` title-bar action button must set the raw
    /// `MouseDown`'s `widget` to the action's own id — exercising
    /// `crate::render::paint_layout`'s `LayoutNode::Panel` arm, which
    /// records the panel's chrome layout as a [`crate::hit::PaintedHit::Panel`]
    /// so this hit resolves instead of being dropped, same as every other
    /// primitive's own hit-test coverage in this module.
    #[test]
    fn clicking_a_panel_action_button_sets_the_click_widget_to_its_id() {
        use quadraui::{Panel, PanelAction, StyledText};

        let panel = Panel {
            id: WidgetId::new("outer"),
            title: Some(StyledText::plain("Explorer")),
            actions: vec![PanelAction {
                id: WidgetId::new("close"),
                icon: "x".to_string(),
                tooltip: "Close".to_string(),
                is_active: false,
            }],
            accent: None,
            collapsed: false,
        };
        let logic = PanelLogic {
            panel,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 10);
        driver.render();

        // The lone action button is right-aligned in the title bar — TUI's
        // own 3-cell action width (`TUI_ACTION_BUTTON_WIDTH` in
        // `quadraui::tui::panel`) puts it at [40.0 - 3.0, 40.0) ==
        // [37.0, 40.0).
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(38.0, 0.0),
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::MouseDown { widget: Some(id), .. } if *id == WidgetId::new("close")
            )),
            "expected a MouseDown with widget=close: {:?}",
            dispatched
        );
    }

    /// Clicking a `Panel`'s title bar away from its action buttons (the
    /// collapse affordance — `PanelHit::TitleBar` is the only collapse hit
    /// the primitive exposes) must set the raw `MouseDown`'s `widget` to
    /// the panel's own id.
    #[test]
    fn clicking_a_panel_title_bar_sets_the_click_widget_to_the_panel_id() {
        use quadraui::{Panel, StyledText};

        let panel = Panel {
            id: WidgetId::new("outer"),
            title: Some(StyledText::plain("Explorer")),
            actions: vec![],
            accent: None,
            collapsed: false,
        };
        let logic = PanelLogic {
            panel,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 10);
        driver.render();

        // Row 0 away from any action button (there are none here) is the
        // title bar itself.
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(1.0, 0.0),
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::MouseDown { widget: Some(id), .. } if *id == WidgetId::new("outer")
            )),
            "expected a MouseDown with widget=outer: {:?}",
            dispatched
        );
    }

    /// A minimal [`AppLogic`] painting a single `quadraui::SidebarPanel`
    /// node with a toolbar, for the toolbar-button-click coverage below.
    struct SidebarPanelLogic {
        sidebar_panel: quadraui::SidebarPanel,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for SidebarPanelLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let node = crate::layout::LayoutNode::SidebarPanel(crate::layout::LayoutSidebarPanel {
                sidebar_panel: self.sidebar_panel.clone(),
                content: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "body".to_string(),
                    descriptor: LeafDescriptor::Button(Button::new(
                        WidgetId::new("body-btn"),
                        "Body",
                    )),
                    scroll_px: None,
                })),
            });
            paint_layout(backend, area, &node, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Clicking a `SidebarPanel`'s toolbar button must deliver a typed
    /// `UiEvent::Toolbar(bar_id, ToolbarEvent::ButtonClicked { id, is_active })`
    /// — exercising `crate::render::paint_layout`'s `LayoutNode::SidebarPanel`
    /// arm, which records the panel's chrome layout (and its embedded
    /// `Toolbar` descriptor) as a [`crate::hit::PaintedHit::SidebarPanel`] so
    /// this hit resolves through `semantic_event_for_hit`'s
    /// `HitDetail::SidebarPanelToolbarButton` arm instead of staying a raw
    /// `MouseDown`.
    #[test]
    fn clicking_a_sidebar_panel_toolbar_button_dispatches_a_toolbar_button_clicked_event() {
        use quadraui::{SidebarPanel, Toolbar, ToolbarButton};

        let sidebar_panel = SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: Some(Toolbar::new(WidgetId::new("sb:toolbar")).with_buttons(vec![
                ToolbarButton::Action {
                    spec: None,
                    id: WidgetId::new("refine"),
                    label: "Refine".to_string(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                },
            ])),
            toolbar_height: None,
        };
        let logic = SidebarPanelLogic {
            sidebar_panel,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 10);
        driver.render();

        // The lone button starts at the toolbar's own x=0, row 0.
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(1.0, 0.0),
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::Toolbar(
                    bar,
                    ToolbarEvent::ButtonClicked { id, is_active: true },
                ) if *bar == WidgetId::new("sb:toolbar") && *id == WidgetId::new("refine")
            )),
            "expected a Toolbar(sb:toolbar, ButtonClicked{{id: refine, is_active: true}}): {:?}",
            dispatched
        );
    }

    /// Mirrors [`FormLogic`], but paints one standalone `"toolbar"` extra —
    /// the leaf/standalone shape ([`crate::hit::HitDetail::ExtraChrome`]),
    /// distinct from the `SidebarPanel`-embedded shape the test above
    /// covers — so a test can drive its click and Enter/Space keyboard
    /// activation with no host-language FFI type in scope.
    struct ToolbarLogic {
        toolbar: quadraui::Toolbar,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for ToolbarLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let mut extra: HashMap<String, std::rc::Rc<dyn crate::extra_paint::ExtraPaint>> =
                HashMap::new();
            extra.insert(
                "toolbar".to_string(),
                std::rc::Rc::new(self.toolbar.clone())
                    as std::rc::Rc<dyn crate::extra_paint::ExtraPaint>,
            );
            draw_bands(
                backend,
                RenderRefs {
                    button: None,
                    text_display: None,
                    list: None,
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
                &|id| self.engine.chrome_interaction(id),
            );
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    fn two_action_toolbar() -> quadraui::Toolbar {
        use quadraui::{Toolbar, ToolbarButton};

        Toolbar::new(WidgetId::new("toolbar:main")).with_buttons(vec![
            ToolbarButton::Action {
                spec: None,
                id: WidgetId::new("filter"),
                label: "Filter".to_string(),
                icon: None,
                key_hint: None,
                enabled: true,
                is_active: false,
                tooltip: String::new(),
            },
            ToolbarButton::Action {
                spec: None,
                id: WidgetId::new("reset"),
                label: "Reset".to_string(),
                icon: None,
                key_hint: None,
                enabled: true,
                is_active: false,
                tooltip: String::new(),
            },
        ])
    }

    /// A click on a standalone toolbar's `Action` button must dispatch a
    /// typed `UiEvent::Toolbar(bar_id, ToolbarEvent::ButtonClicked)` —
    /// `is_active` toggling `true` proves the "new state" half of the
    /// contract, not just the id.
    #[test]
    fn clicking_a_standalone_toolbar_button_dispatches_button_clicked_with_the_flipped_state() {
        let logic = ToolbarLogic {
            toolbar: two_action_toolbar(),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 5);
        driver.render();

        // "Filter" is the first button, starting at the toolbar's own x=0.
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(1.0, 0.0),
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        // No leading `FocusChanged`, unlike a tab-stoppable fixed-slot
        // primitive's own click (e.g. `click_on_a_button_focuses_it_then_
        // dispatches_the_click` above): the *clicked button's* id (not the
        // bar's) is what resolves against the hit map, and only the bar's
        // id is a tab stop, so click-to-focus never fires here.
        assert_eq!(
            dispatched.len(),
            1,
            "expected exactly the Toolbar click, no FocusChanged: {:?}",
            dispatched
        );
        assert!(
            matches!(
                &dispatched[0].0,
                UiEvent::Toolbar(
                    bar,
                    ToolbarEvent::ButtonClicked { id, is_active: true },
                ) if *bar == WidgetId::new("toolbar:main") && *id == WidgetId::new("filter")
            ),
            "expected Toolbar(toolbar:main, ButtonClicked{{id: filter, is_active: true}}): {:?}",
            dispatched
        );
    }

    /// Tab must land focus on a standalone toolbar (it now opts into
    /// [`crate::extra_paint::ExtraPaint::tab_stop`] whenever it has at least
    /// one enabled `Action`), and Enter on the focused toolbar must activate
    /// whichever button [`quadraui::Toolbar::focused_index`] names — the
    /// same `UiEvent::Toolbar(ButtonClicked)` shape a mouse click produces.
    #[test]
    fn tab_focuses_a_toolbar_and_enter_activates_its_focused_button() {
        // `focused_index: Some(1)` simulates the host having already
        // responded to the prior `FocusChanged` by marking "Reset" (the
        // second button) as the keyboard-focused one — the same
        // host-driven convention `Switch::focused` documents.
        let mut toolbar = two_action_toolbar();
        toolbar.focused_index = Some(1);

        let logic = ToolbarLogic {
            toolbar,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 5);
        driver.render();

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert!(
                dispatched.iter().any(|(event, _)| matches!(
                    event,
                    UiEvent::FocusChanged(Some(id)) if *id == WidgetId::new("toolbar:main")
                )),
                "Tab must focus the toolbar (its only tab stop): {:?}",
                dispatched
            );
        }

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::Toolbar(
                    bar,
                    ToolbarEvent::ButtonClicked { id, is_active: true },
                ) if *bar == WidgetId::new("toolbar:main") && *id == WidgetId::new("reset")
            )),
            "Enter on the focused toolbar must activate button 1 (reset): {:?}",
            dispatched
        );
    }

    /// `message_dialog_options_from_buttons` carries each button's
    /// `DialogButton::spec` through to the built `MessageDialogButton`
    /// unchanged — it must not silently drop it the way it used to.
    #[test]
    fn message_dialog_options_from_buttons_carries_spec_through() {
        use quadraui::{ButtonSpec, ButtonVariant, DialogButton};

        let mut spec = ButtonSpec::new("Delete");
        spec.variant = ButtonVariant::Danger;
        let buttons = vec![DialogButton {
            id: WidgetId::new("delete"),
            label: "Delete".to_string(),
            is_default: true,
            is_cancel: false,
            tint: None,
            spec: Some(spec.clone()),
        }];
        let opts = super::message_dialog_options_from_buttons(
            "T".to_string(),
            "B".to_string(),
            buttons,
            None,
        )
        .expect("no tint set — must be accepted");
        assert_eq!(opts.buttons[0].spec, Some(spec));
    }

    /// A button with no `spec` override still builds cleanly, with
    /// `MessageDialogButton::spec` left `None`.
    #[test]
    fn message_dialog_options_from_buttons_leaves_spec_none_by_default() {
        use quadraui::DialogButton;

        let buttons = vec![DialogButton {
            id: WidgetId::new("ok"),
            label: "OK".to_string(),
            is_default: true,
            is_cancel: false,
            tint: None,
            spec: None,
        }];
        let opts = super::message_dialog_options_from_buttons(
            "T".to_string(),
            "B".to_string(),
            buttons,
            None,
        )
        .expect("no tint set — must be accepted");
        assert_eq!(opts.buttons[0].spec, None);
    }

    /// A column of `count` buttons, labelled `"0"`..`"{count - 1}"` by
    /// index — mirrors `crate::render::tests::button_column` /
    /// `crate::host::tests::ScrollLeafLogic`'s own content builder, the
    /// shared content every vertical-axis `LayoutNode::Scroll` input test
    /// below stacks.
    fn button_column(count: usize) -> LayoutNode {
        LayoutNode::Column(LayoutContainer::new(
            (0..count)
                .map(|i| {
                    let label = i.to_string();
                    LayoutNode::Leaf(LayoutLeaf {
                        id: label.clone(),
                        descriptor: LeafDescriptor::Button(Button::new(
                            WidgetId::new(label.clone()),
                            label,
                        )),
                        scroll_px: None,
                    })
                })
                .collect(),
            Spacing::default(),
        ))
    }

    /// [`button_column`]'s horizontal-axis counterpart: a `Row` of `count`
    /// buttons, same labelling — the content a horizontal-axis
    /// `LayoutNode::Scroll` input test stacks.
    fn button_row(count: usize) -> LayoutNode {
        LayoutNode::Row(LayoutContainer::new(
            (0..count)
                .map(|i| {
                    let label = i.to_string();
                    LayoutNode::Leaf(LayoutLeaf {
                        id: label.clone(),
                        descriptor: LeafDescriptor::Button(Button::new(
                            WidgetId::new(label.clone()),
                            label,
                        )),
                        scroll_px: None,
                    })
                })
                .collect(),
            Spacing::default(),
        ))
    }

    /// An [`AppLogic`] whose `render`/`handle` go through
    /// [`InteractionEngine`] exactly the way [`MinimalLogic`] does, hosting
    /// a `LayoutNode::Scroll` around [`button_column`] (vertical `axis`)
    /// or [`button_row`] (horizontal) instead of a bare `Button` — the
    /// issue's own acceptance scenario (40 buttons, a 10-row viewport)
    /// driven through the full `event -> hit -> semantic` pipeline, not
    /// just [`crate::render::paint_layout`]/
    /// [`crate::host::resolve_layout_node`] called directly the way
    /// `crate::render::tests::ScrollButtonsLogic`/
    /// `crate::host::tests::ScrollLeafLogic` already cover.
    struct ScrollLogic {
        button_count: usize,
        offset: Option<f32>,
        axis: ScrollAxis,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for ScrollLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let content = match self.axis {
                ScrollAxis::Vertical => button_column(self.button_count),
                ScrollAxis::Horizontal => button_row(self.button_count),
            };
            let mut node = LayoutNode::Scroll(LayoutScroll {
                id: "scroll".to_string(),
                axis: self.axis,
                offset: self.offset,
                content: Box::new(content),
                spacing: Spacing::default(),
            });
            resolve_layout_node(self.engine.widget_state(), backend, area, &mut node);
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Builds a `button_count`-button vertical-axis `Scroll` (`offset` as
    /// given — `None` for uncontrolled) and drives it through a 10-row
    /// [`TuiDriver`], mirroring [`tree_driver`]/`data_table_driver`.
    fn scroll_driver(button_count: usize, offset: Option<f32>) -> TuiDriver<ScrollLogic> {
        let logic = ScrollLogic {
            button_count,
            offset,
            axis: ScrollAxis::Vertical,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        TuiDriver::new(logic, 20, 10)
    }

    /// [`scroll_driver`]'s horizontal-axis counterpart — same
    /// `button_count`/`offset` contract, [`button_row`] content, through
    /// the same 20-column viewport (each button is 5 columns wide per
    /// [`crate::render::natural_size`]'s `Button` arm at `char_width() <=
    /// 1.0`, so the viewport shows 4 at a time).
    fn horizontal_scroll_driver(
        button_count: usize,
        offset: Option<f32>,
    ) -> TuiDriver<ScrollLogic> {
        let logic = ScrollLogic {
            button_count,
            offset,
            axis: ScrollAxis::Horizontal,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        TuiDriver::new(logic, 20, 10)
    }

    /// A vertical-axis `Scroll` content tree a page-key scoping test
    /// needs: a focused leaf other than a plain `Button` mixed in among
    /// enough of them that the enclosing `Scroll` has room to scroll
    /// (`max_offset > 0`) — proving a page key reaches that leaf's own
    /// handling rather than paging the container just because a `Scroll`
    /// COULD page. `leaf` is placed first (row 0, so it starts visible at
    /// a zero offset) and `tail_buttons` fills out the rest.
    fn leaf_then_buttons_column(leaf: LayoutNode, tail_buttons: usize) -> LayoutNode {
        let mut children = vec![leaf];
        children.extend((0..tail_buttons).map(|i| {
            let label = i.to_string();
            LayoutNode::Leaf(LayoutLeaf {
                id: label.clone(),
                descriptor: LeafDescriptor::Button(Button::new(
                    WidgetId::new(label.clone()),
                    label,
                )),
                scroll_px: None,
            })
        }));
        LayoutNode::Column(LayoutContainer::new(children, Spacing::default()))
    }

    /// [`leaf_then_buttons_column`]'s content: a focused `TextInput`
    /// (id `"input"`, one line `"hello"`, cursor at column 0) followed by
    /// 14 buttons — 15 rows total in a 10-row viewport, so the enclosing
    /// `Scroll` has 5 rows of room to page.
    fn text_input_then_buttons_column() -> LayoutNode {
        let text_input = TextInput::new(WidgetId::new("input"))
            .with_lines(vec!["hello".to_string()])
            .with_cursor_col(0);
        leaf_then_buttons_column(
            LayoutNode::Leaf(LayoutLeaf {
                id: "input".to_string(),
                descriptor: LeafDescriptor::Extra(Rc::new(text_input)),
                scroll_px: None,
            }),
            14,
        )
    }

    /// [`leaf_then_buttons_column`]'s content: a focused `TextDisplay`
    /// (id `"log"`, one line) followed by 14 buttons — same 15-rows-in-a
    /// 10-row-viewport shape as [`text_input_then_buttons_column`].
    fn text_display_then_buttons_column() -> LayoutNode {
        let text_display = TextDisplay {
            id: WidgetId::new("log"),
            lines: vec![TextDisplayLine {
                spans: vec![StyledSpan::plain("line 0")],
                decoration: Default::default(),
                timestamp: None,
            }],
            scroll_offset: 0,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: false,
        };
        leaf_then_buttons_column(
            LayoutNode::Leaf(LayoutLeaf {
                id: "log".to_string(),
                descriptor: LeafDescriptor::TextDisplay(text_display),
                scroll_px: None,
            }),
            14,
        )
    }

    /// An [`AppLogic`] hosting a vertical `LayoutNode::Scroll` around
    /// whatever `content_builder` returns — [`ScrollLogic`]'s own
    /// button-only content can't host a `TextInput`/`TextDisplay` leaf,
    /// which the page-key scoping tests need mixed in among buttons.
    struct ScrollWithLeafLogic {
        content_builder: fn() -> LayoutNode,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for ScrollWithLeafLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let mut node = LayoutNode::Scroll(LayoutScroll {
                id: "scroll".to_string(),
                axis: ScrollAxis::Vertical,
                offset: None,
                content: Box::new((self.content_builder)()),
                spacing: Spacing::default(),
            });
            resolve_layout_node(self.engine.widget_state(), backend, area, &mut node);
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Builds a [`ScrollWithLeafLogic`] and drives it through a 10-row
    /// [`TuiDriver`], mirroring [`scroll_driver`].
    fn scroll_with_leaf_driver(
        content_builder: fn() -> LayoutNode,
    ) -> TuiDriver<ScrollWithLeafLogic> {
        let logic = ScrollWithLeafLogic {
            content_builder,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        TuiDriver::new(logic, 20, 10)
    }

    fn painted_button_ids(hits: &[PaintedHit]) -> Vec<String> {
        hits.iter()
            .filter_map(|hit| match hit {
                PaintedHit::Button { id, .. } => Some(id.as_str().to_string()),
                _ => None,
            })
            .collect()
    }

    /// A wheel `Scroll` over an uncontrolled `Scroll` container moves its
    /// own stored offset — 40 buttons, a 10-row viewport, 5 notches down
    /// (`ScrollDelta::new(0.0, 5.0)`, one row per notch at TUI's
    /// `line_height() == 1.0`) must bring button `"5"` to the top of the
    /// viewport.
    #[test]
    fn wheel_scroll_over_an_uncontrolled_scroll_container_moves_its_own_offset() {
        let mut driver = scroll_driver(40, None);
        driver.render();
        driver.dispatch(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta::new(0.0, 5.0),
            position: Point { x: 0.0, y: 0.0 },
        });
        driver.render();
        let hits = driver.app().engine.hit_map();
        let want: Vec<String> = (5..15).map(|i| i.to_string()).collect();
        assert_eq!(painted_button_ids(&hits), want);
    }

    /// PageDown with focus on a button inside an uncontrolled `Scroll`
    /// scrolls the container by one viewport (10 rows), landing button
    /// `"10"` at the top — then a `MouseDown` at the viewport's own
    /// top-left row resolves to that same button, matching the issue's
    /// own "click the button now at the top, and see the right id"
    /// acceptance step.
    #[test]
    fn page_down_scrolls_the_container_and_the_button_now_on_top_is_clickable() {
        let mut driver = scroll_driver(40, None);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("0"))),
                "button \"0\" is a tab stop and must accept focus"
            );
        }
        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::PageDown),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        driver.render();
        {
            let hits = driver.app().engine.hit_map();
            let want: Vec<String> = (10..20).map(|i| i.to_string()).collect();
            assert_eq!(painted_button_ids(&hits), want);
        }

        driver.app().sink.borrow_mut().dispatched.clear();
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            position: Point { x: 0.0, y: 0.0 },
        });
        let clicked =
            driver
                .app()
                .sink
                .borrow()
                .dispatched
                .iter()
                .find_map(|(event, detail)| match (event, detail) {
                    (
                        UiEvent::MouseDown {
                            widget: Some(id), ..
                        },
                        Some(HitDetail::ButtonBody),
                    ) => Some(id.as_str().to_string()),
                    _ => None,
                });
        assert_eq!(
            clicked,
            Some("10".to_string()),
            "the button now painted at the viewport's top must be the one that gets clicked"
        );
    }

    /// Dragging an uncontrolled `Scroll`'s scrollbar thumb all the way to
    /// the bottom of its track brings the LAST button into view, matching
    /// the issue's own "drag the thumb to the bottom and get the last
    /// button visible" acceptance step.
    #[test]
    fn dragging_the_scroll_thumb_to_the_bottom_reveals_the_last_button() {
        let mut driver = scroll_driver(40, None);
        driver.render();
        let track = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    PaintedHit::Scroll {
                        id,
                        scrollbar: Some(scrollbar),
                        ..
                    } if *id == WidgetId::new("scroll") => Some(scrollbar.track),
                    _ => None,
                })
                .expect("40 buttons in a 10-row viewport must paint a scrollbar")
        };

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            position: Point {
                x: track.x,
                y: track.y,
            },
        });
        driver.dispatch(UiEvent::MouseMoved {
            position: Point {
                x: track.x,
                y: track.y + track.height,
            },
            buttons: quadraui::ButtonMask {
                left: true,
                ..Default::default()
            },
        });
        driver.dispatch(UiEvent::MouseUp {
            widget: None,
            button: MouseButton::Left,
            position: Point {
                x: track.x,
                y: track.y + track.height,
            },
        });
        driver.render();

        let hits = driver.app().engine.hit_map();
        let painted = painted_button_ids(&hits);
        assert_eq!(
            painted.last(),
            Some(&"39".to_string()),
            "dragging the thumb to the bottom of the track must reveal the last button: {painted:?}"
        );
    }

    /// A controlled `Scroll` (descriptor pins `offset`) never actually
    /// moves from a wheel `Scroll` — the pinned value wins again the
    /// moment the next frame re-resolves it — but the engine still emits
    /// `ScrollOffsetChanged` for the app to see, matching the issue's own
    /// "a controlled offset does not move without the app's say-so but
    /// still emits `ScrollOffsetChanged`" acceptance step.
    #[test]
    fn controlled_scroll_offset_is_pinned_but_still_emits_scroll_offset_changed() {
        let mut driver = scroll_driver(40, Some(20.0));
        driver.render();
        driver.dispatch(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta::new(0.0, 5.0),
            position: Point { x: 0.0, y: 0.0 },
        });

        let emitted =
            driver
                .app()
                .sink
                .borrow()
                .dispatched
                .iter()
                .find_map(|(event, _)| match event {
                    UiEvent::ScrollOffsetChanged { widget, new_offset }
                        if *widget == WidgetId::new("scroll") =>
                    {
                        Some(*new_offset)
                    }
                    _ => None,
                });
        assert_eq!(
            emitted,
            Some(25),
            "a wheel scroll over a controlled Scroll must still emit ScrollOffsetChanged: {:?}",
            emitted
        );

        driver.render();
        let hits = driver.app().engine.hit_map();
        let want: Vec<String> = (20..30).map(|i| i.to_string()).collect();
        assert_eq!(
            painted_button_ids(&hits),
            want,
            "a controlled offset must stay pinned at the descriptor's own value"
        );
    }

    /// A directly-dispatched `UiEvent::FocusChanged` — the shape a
    /// binding's own Tab-cycling sends straight into `AppLogic::handle`,
    /// never going through this engine's click-to-focus bookkeeping —
    /// still scrolls an uncontrolled `Scroll` so the newly-focused child
    /// (button `"25"`, scrolled fully out of the initial `[0, 10)`
    /// viewport) ends up fully visible.
    #[test]
    fn a_direct_focus_changed_event_scrolls_the_newly_focused_child_into_view() {
        let mut driver = scroll_driver(40, None);
        driver.render();
        {
            let hits = driver.app().engine.hit_map();
            let want: Vec<String> = (0..10).map(|i| i.to_string()).collect();
            assert_eq!(painted_button_ids(&hits), want);
        }

        driver.dispatch(UiEvent::FocusChanged(Some(WidgetId::new("25"))));
        driver.render();

        let hits = driver.app().engine.hit_map();
        let painted = painted_button_ids(&hits);
        assert!(
            painted.contains(&"25".to_string()),
            "button \"25\" must now be painted (hence visible) after receiving focus: {painted:?}"
        );
    }

    /// A horizontal-axis `Scroll` routes a wheel's `delta.x` and a thumb
    /// drag's `position.x` the same way the vertical tests above exercise
    /// `delta.y`/`position.y` — covering `ScrollDrag::axis`, the
    /// `position.x` continuation in the drag-move branch, `char_width()`
    /// as the horizontal wheel step, and `scrollbar.axis` in the
    /// thumb-arming path, none of which any other test here reaches.
    #[test]
    fn horizontal_scroll_routes_wheel_and_thumb_drag_through_delta_x_and_position_x() {
        let mut driver = horizontal_scroll_driver(8, None);
        driver.render();
        let initial = painted_button_ids(&driver.app().engine.hit_map());
        assert_eq!(
            initial.first(),
            Some(&"0".to_string()),
            "must start scrolled to the first button: {initial:?}"
        );

        driver.dispatch(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta::new(5.0, 0.0),
            position: Point { x: 0.0, y: 0.0 },
        });
        driver.render();
        let after_wheel = painted_button_ids(&driver.app().engine.hit_map());
        assert_ne!(
            after_wheel.first(),
            Some(&"0".to_string()),
            "a horizontal wheel delta.x must move the container's own offset: {after_wheel:?}"
        );

        let scrollbar = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    PaintedHit::Scroll {
                        id,
                        scrollbar: Some(scrollbar),
                        ..
                    } if *id == WidgetId::new("scroll") => Some(scrollbar.clone()),
                    _ => None,
                })
                .expect("8 buttons wider than a 20-column viewport must paint a scrollbar")
        };
        assert_eq!(scrollbar.axis, ScrollAxis::Horizontal);

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            position: Point {
                x: scrollbar.track.x + scrollbar.thumb_start,
                y: scrollbar.track.y,
            },
        });
        driver.dispatch(UiEvent::MouseMoved {
            position: Point {
                x: scrollbar.track.x + scrollbar.track.width,
                y: scrollbar.track.y,
            },
            buttons: quadraui::ButtonMask {
                left: true,
                ..Default::default()
            },
        });
        driver.dispatch(UiEvent::MouseUp {
            widget: None,
            button: MouseButton::Left,
            position: Point {
                x: scrollbar.track.x + scrollbar.track.width,
                y: scrollbar.track.y,
            },
        });
        driver.render();

        let hits = driver.app().engine.hit_map();
        let painted = painted_button_ids(&hits);
        assert_eq!(
            painted.last(),
            Some(&"7".to_string()),
            "dragging the thumb to the end of a horizontal track must reveal the last button: {painted:?}"
        );
    }

    /// A press on a `Scroll`'s own scrollbar track (not the thumb) pages
    /// by one viewport — the `HitDetail::ScrollScrollbarTrackBefore`/
    /// `TrackAfter` arm, otherwise exercised by no test here. Pressing
    /// below the thumb (`PressZone::After`, with the thumb starting at
    /// the track's top since the container starts unscrolled) must page
    /// the container down by one viewport, bringing button `"10"` to the
    /// top — mirroring [`dragging_the_scroll_thumb_to_the_bottom_reveals_the_last_button`]'s
    /// own track lookup, but clicking (not dragging) a point past the
    /// thumb's end instead of on it.
    #[test]
    fn pressing_the_scroll_track_after_the_thumb_pages_down_by_one_viewport() {
        let mut driver = scroll_driver(40, None);
        driver.render();
        let track = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    PaintedHit::Scroll {
                        id,
                        scrollbar: Some(scrollbar),
                        ..
                    } if *id == WidgetId::new("scroll") => Some(scrollbar.track),
                    _ => None,
                })
                .expect("40 buttons in a 10-row viewport must paint a scrollbar")
        };

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            position: Point {
                x: track.x,
                y: track.y + track.height - 0.5,
            },
        });
        driver.render();

        let hits = driver.app().engine.hit_map();
        let want: Vec<String> = (10..20).map(|i| i.to_string()).collect();
        assert_eq!(
            painted_button_ids(&hits),
            want,
            "pressing the track after the thumb must page the container down by one viewport"
        );
    }

    /// `End` on a focused `TextInput` that is a direct child of a `Scroll`
    /// with room to page (`max_offset > 0`) must move the caret, never
    /// page the enclosing container — `focused_leaf_claims_page_keys`'s
    /// whole reason to exist. The `TextInput` starts with its cursor at
    /// column 0 of a non-empty line, so `End` is a real cursor move (not a
    /// no-op the old, ungated routing could coincidentally get right), and
    /// moving it changes no text (`EditOp::MoveLineEnd` never touches
    /// `lines`), so [`crate::text_input::apply_text_input_edit`] returns
    /// `None` and [`crate::semantic::focus_scoped_key_event`] falls back
    /// to the raw `KeyPressed` — the signal this test checks for, rather
    /// than a `ScrollOffsetChanged` the old routing would have emitted
    /// instead (and returned early on, never reaching this fallback).
    #[test]
    fn end_on_a_focused_text_input_inside_a_scroll_moves_the_caret_not_the_scroll() {
        let mut driver = scroll_with_leaf_driver(text_input_then_buttons_column);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            backend.set_focus(Some(WidgetId::new("input")));
        }

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::End),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let sink = driver.app().sink.borrow();
        assert!(
            !sink
                .dispatched
                .iter()
                .any(|(event, _)| matches!(event, UiEvent::ScrollOffsetChanged { .. })),
            "End on a focused TextInput inside a Scroll must never page the container: {:?}",
            sink.dispatched
        );
        assert!(
            sink.dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::KeyPressed {
                    key: Key::Named(NamedKey::End),
                    ..
                }
            )),
            "the raw KeyPressed must still reach the binding once the TextInput's own \
             cursor-only move leaves `focus_scoped_key_event` with nothing to substitute: {:?}",
            sink.dispatched
        );
    }

    /// `PageDown` on a focused `TextDisplay` that is a direct child of a
    /// `Scroll` with room to page must route into the `TextDisplay`'s own
    /// `UiEvent::TextDisplay(.., KeyPressed)`, never page the enclosing
    /// container — `PaintedHit::TextDisplay` is always a tab stop and
    /// always claims its own keys via `focus_scoped_key_event`, the other
    /// half of `focused_leaf_claims_page_keys`'s gate.
    #[test]
    fn page_down_on_a_focused_text_display_inside_a_scroll_routes_to_the_text_display_not_the_scroll(
    ) {
        let mut driver = scroll_with_leaf_driver(text_display_then_buttons_column);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            backend.set_focus(Some(WidgetId::new("log")));
        }

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::PageDown),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let sink = driver.app().sink.borrow();
        assert!(
            !sink
                .dispatched
                .iter()
                .any(|(event, _)| matches!(event, UiEvent::ScrollOffsetChanged { .. })),
            "PageDown on a focused TextDisplay inside a Scroll must never page the container: {:?}",
            sink.dispatched
        );
        assert!(
            sink.dispatched
                .iter()
                .any(|(event, _)| matches!(event, UiEvent::TextDisplay(id, _) if *id == WidgetId::new("log"))),
            "PageDown must reach the TextDisplay's own KeyPressed handling: {:?}",
            sink.dispatched
        );
    }

    /// A standalone `"text_input"` leaf whose `render` rebuilds the
    /// descriptor from `value` every frame but — unlike every other
    /// `AppLogic` in this module's own test suite, and unlike a
    /// well-behaved real binding — deliberately never echoes
    /// `TextInput::has_focus` back as `true` once the engine focuses it.
    /// Regression fixture for the "engine focus is authoritative,
    /// `has_focus` is cosmetic" contract
    /// [`crate::text_input`]'s own module doc states: key routing for a
    /// focused `"text_input"`/`"password"`/`"textarea"` extra is keyed
    /// entirely by `backend.focus_manager().focused()`
    /// ([`focus_scoped_key_event`]/`text_input_insert_event`'s own
    /// `focused` parameter), never by the descriptor's own `has_focus`
    /// field, so a host that never echoes it still keeps every keystroke
    /// reaching the cached [`quadraui::TextEditor`] and being applied to
    /// `value` via `FormEvent::TextInputChanged`.
    struct NeverEchoesFocusTextInputLogic {
        value: RefCell<String>,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for NeverEchoesFocusTextInputLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let text_input = TextInput::new(WidgetId::new("input"))
                .with_lines(vec![self.value.borrow().clone()])
                .with_cursor_col(self.value.borrow().chars().count());
            let node = LayoutNode::Leaf(LayoutLeaf {
                id: "input".to_string(),
                descriptor: LeafDescriptor::Extra(std::rc::Rc::new(text_input)),
                scroll_px: None,
            });
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            let r = self
                .engine
                .handle(event, backend, &mut *self.sink.borrow_mut());
            let dispatched = self.sink.borrow().dispatched.clone();
            for (ev, _) in dispatched {
                if let UiEvent::Form(_, quadraui::FormEvent::TextInputChanged { value, .. }) = ev {
                    *self.value.borrow_mut() = value;
                }
            }
            r
        }
    }

    /// Three keystrokes into a `"text_input"` extra the engine already
    /// focuses (via a direct `backend.set_focus`, mirroring a host's own
    /// click-to-focus or Tab-cycling outcome) must all land, in order,
    /// even though [`NeverEchoesFocusTextInputLogic::render`] never once
    /// echoes `has_focus: true` back onto the descriptor across the three
    /// re-renders between them. A regression on the "has_focus gates
    /// routing" shape this guards against would drop every keystroke
    /// after the first — see this test's own fixture doc.
    #[test]
    fn typing_into_a_focused_text_input_never_drops_a_key_when_has_focus_is_never_echoed() {
        let logic = NeverEchoesFocusTextInputLogic {
            value: RefCell::new(String::new()),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 20, 5);
        driver.render();

        {
            let (backend, _app) = driver.parts_mut();
            backend.set_focus(Some(WidgetId::new("input")));
        }
        driver.render();

        for ch in ['a', 'b', 'c'] {
            driver.dispatch(UiEvent::KeyPressed {
                key: Key::Char(ch),
                modifiers: Modifiers::default(),
                repeat: false,
            });
            driver.render();
        }

        assert_eq!(
            *driver.app().value.borrow(),
            "abc",
            "every keystroke must land even though has_focus is never echoed back: {:?}",
            driver.app().sink.borrow().dispatched
        );
    }

    /// Same contract as
    /// [`typing_into_a_focused_text_input_never_drops_a_key_when_has_focus_is_never_echoed`],
    /// but focusing the input through a real `MouseDown` click-to-focus
    /// (exercising `InteractionEngine::handle`'s own focus-setting path)
    /// rather than a direct `backend.set_focus` — the two keystrokes right
    /// after the click must still both land.
    #[test]
    fn click_to_focus_a_text_input_then_typing_never_drops_a_key_when_has_focus_is_never_echoed() {
        let logic = NeverEchoesFocusTextInputLogic {
            value: RefCell::new(String::new()),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 20, 5);
        driver.render();

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(1.0, 0.0),
            modifiers: Modifiers::default(),
        });
        driver.render();

        for ch in ['x', 'y'] {
            driver.dispatch(UiEvent::KeyPressed {
                key: Key::Char(ch),
                modifiers: Modifiers::default(),
                repeat: false,
            });
            driver.render();
        }

        assert_eq!(
            *driver.app().value.borrow(),
            "xy",
            "every keystroke after click-to-focus must land even though has_focus is \
             never echoed back: {:?}",
            driver.app().sink.borrow().dispatched
        );
    }
}
