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
use std::rc::Rc;

use quadraui::backend::MessageDialogOptions;
use quadraui::compose::{
    AppMenu, AppMenuEvent, AppMenuState, ContextMenuController, ContextMenuOutcome, FormController,
    FormControllerAction, FormDialog, FormDialogState, Handled, MenuDef, MessageDialog,
    TreeTableController, TreeTableControllerEvent,
};
use quadraui::primitive::DynWidget;
use quadraui::primitives::form::focus_order;
use quadraui::primitives::scrollbar::{scroll_by_clamped, ThumbDrag};
use quadraui::DrawsPrimitives;
use quadraui::{
    wheel_row_delta, Backend, Button, CellGrid, CellGridEvent, CellGridPrimitiveLayout, Column,
    CompletionItemMeasure, Completions, CompletionsLayout, ContextMenu, ContextMenuItem,
    ContextMenuPlacement, DataTableEvent, Dialog, EditOp, EventCx, EventHost, Float, FloatLayout,
    FloatMeasure, Form, FormEvent, Key, KeyEvent, ListViewEvent, MenuBar, MessageDialogButton,
    Modifiers, MouseButton, NamedKey, Point, PointerKind, PointerShape, Rect, RichTextPopup,
    RichTextPopupLayout, RoutedEvent, ScrollAxis, ScrollViewEvent, TabBarEvent, TextDisplayEvent,
    TextInput, Toast, ToastCorner, ToastHit, ToastOverlay, ToastStackLayout, Tooltip,
    TooltipLayout, TooltipMeasure, TreeEvent, TreeTableNodeData, UiEvent, WidgetId,
};

use crate::drag::{
    enclosing_scroll_child, find_data_table_layout, find_split_layout, find_split_tree_layout,
    gesture_target_at, scroll_scrollbar, scroll_view_at, text_display_scroll_state,
    tree_scroll_state, ActiveDrag, DataTableColumnDrag, GestureDrag, ScrollDrag, ScrollbarDrag,
};
use crate::extra_paint::{extra_as, TreeTableControllerDescriptor};
use crate::hit::{
    data_table_rows_and_viewport, event_position, hit_test_painted, list_rows_and_viewport,
    set_event_widget, tree_rows_and_viewport, tree_total_rows_and_viewport, windowed_leaf_extent,
    HitDetail, LoadedRows, PaintedHit,
};
use crate::host::{
    apply_data_table_nav_key, apply_list_nav_key, apply_scroll_offset, apply_split_divider_drag,
    apply_tab_activated, apply_tab_closed, apply_tree_nav_key, apply_tree_nav_key_windowed,
    apply_tree_toggle_expand, stored_cursor_scroll_offset,
};
use crate::layout::LayoutNode;
use crate::render::{paint_layout, rows_requested_for_window, ItemsWindow, RowsRequestedEvent};
use crate::semantic::{focus_scoped_key_event, semantic_event_for_hit, DragGestureWireEvent};
use crate::text_input::{text_input_click_event, text_input_insert_event, TextEditorCache};
use crate::wire::DragRole;

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
        paint_layout(backend, area, &self.node, &mut painted);
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
            let engine_focused = backend.focus_tree().focused_id() == Some(id);
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
            .focus_tree()
            .focused_id()
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
    /// a synthesized [`quadraui::UiEvent::ButtonActivated`] for a
    /// `"button"`-kind [`HitDetail::WidgetBody`],
    /// [`crate::semantic::semantic_event_for_hit`] for a `Tree`/`List`/
    /// `DataTable`/`TabBar`/`Checkbox`/... leaf, the owning
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
                // A `"button"`-kind `WidgetBody` hit is resolved directly
                // here rather than through `InteractionEngine`'s own
                // `dispatch_dyn_widget_click` (whose `dyn_widget_state`/
                // `dyn_widget_capture` caches live on the main engine, not
                // this subtree's own smaller `LayoutContent`): a `Button`
                // leaf nested in a popover/dialog needs nothing beyond a
                // plain click-to-activate, so its activation is
                // synthesized right here. Only the first press of a
                // double-click activates, mirroring `Button::handle`'s own
                // `PointerKind::Down`-only match on the main tree (which
                // ignores `PointerKind::DoubleClick` outright) — without
                // this guard both presses would fire `ButtonActivated`.
                if matches!(detail, HitDetail::WidgetBody) && !double_click {
                    let is_button = dyn_widget_hit(&self.hits.borrow(), id)
                        .is_some_and(|(_, widget)| widget.as_any().is::<Button>());
                    if is_button {
                        let activated = UiEvent::ButtonActivated { widget: id };
                        return Some(
                            reaction
                                .merge(Reaction::Redraw)
                                .merge(sink.dispatch(&activated, Some(&HitDetail::WidgetBody))),
                        );
                    }
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
                );
                let resolved = semantic.unwrap_or_else(|| event.clone());
                Some(
                    reaction
                        .merge(Reaction::Redraw)
                        .merge(sink.dispatch(&resolved, Some(&detail))),
                )
            }
            UiEvent::KeyPressed { key, modifiers, .. } => {
                let focused = backend.focus_tree().focused_id().copied()?;
                if let Some(area) = self.form_leaf_area(focused) {
                    return self.route_form_event(focused, area, event, backend, sink);
                }
                if matches!(key, Key::Named(NamedKey::Escape)) {
                    return None;
                }
                // Mirrors the click arm above: `focus_scoped_key_event` has
                // no `PaintedHit::Widget` arm (a focused `WIDGET_KINDS` leaf
                // in the main tree instead reaches
                // `InteractionEngine::dispatch_dyn_widget_key`, which keys
                // off caches — `dyn_widget_state`/`dyn_widget_capture` —
                // that live on the main engine, not this smaller subtree),
                // so a focused `"button"`-kind leaf needs its own
                // Enter/Space activation synthesized right here too.
                if matches!(key, Key::Named(NamedKey::Enter) | Key::Char(' ')) {
                    let is_button = dyn_widget_hit(&self.hits.borrow(), focused)
                        .is_some_and(|(_, widget)| widget.as_any().is::<Button>());
                    if is_button {
                        let activated = UiEvent::ButtonActivated { widget: focused };
                        return Some(
                            Reaction::Redraw
                                .merge(sink.dispatch(&activated, Some(&HitDetail::WidgetBody))),
                        );
                    }
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
                let focused = backend.focus_tree().focused_id().copied()?;
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
                let focused = backend.focus_tree().focused_id().copied()?;
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
/// different `hits` slice. `Split`/`SplitTree` are chrome around other
/// content, not focusable leaves themselves, so they are skipped; a
/// `SidebarPanel`'s header is a [`PaintedHit::Widget`] whose
/// `FocusSpec::None` keeps it out too; `Extra` is a tab stop only when its
/// descriptor opts in via [`crate::extra_paint::ExtraPaint::tab_stop`]
/// (currently `TextInput`); `TextDisplay` is always one
/// (scroll/copy need keyboard focus the same way a `Tree`/`List` does);
/// every other variant yields its id and bounding `Rect`.
fn tab_stops_from_hits(hits: &[PaintedHit]) -> Vec<(WidgetId, Rect)> {
    hits.iter()
        .filter_map(|hit| match hit {
            PaintedHit::Tree { id, area, .. }
            | PaintedHit::List { id, area, .. }
            | PaintedHit::DataTable { id, area, .. }
            | PaintedHit::TabBar { id, area, .. }
            | PaintedHit::TextDisplay { id, area, .. } => Some((*id, *area)),
            PaintedHit::Split { .. } | PaintedHit::SplitTree { .. } => None,
            // A `Scroll` is chrome around its own content, like
            // `Split` above -- its content's own
            // leaves are the tab stops, reached via their own
            // `PaintedHit` arms once scrolled into view.
            PaintedHit::Scroll { .. } => None,
            PaintedHit::Extra {
                id, area, extra, ..
            } => extra.tab_stop().then_some((*id, *area)),
            // Mirrors `Extra`'s own opt-in above, through `DynWidget`'s
            // own focus contract rather than a bespoke `tab_stop` bool:
            // only `FocusSpec::Stop` (a single, whole-widget stop) counts.
            // `FocusSpec::Children` (per-sub-target stops) has no
            // `PaintedHit::Widget` field to report more than one
            // `(WidgetId, Rect)` from yet — a widget that needs
            // per-sub-target stops extends this arm with a finer-grained
            // `HitDetail::WidgetBody` of its own. `IconGrid`, `CellGrid`,
            // `Switch` and `Button` each report `FocusSpec::Stop`
            // (`CellGrid` only once it has at least one row and column),
            // so each is a tab stop through this generic path.
            PaintedHit::Widget { id, area, widget } => {
                matches!(widget.focus(), quadraui::FocusSpec::Stop).then_some((*id, *area))
            }
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
// already resolves through `WidgetStateStore::resolve` directly — see
// `host::resolve_scroll_offset`. `resolve`'s own generic signature takes
// `Option<V>` as "is this controlled"; here `V` is itself `Option<WidgetId>`
// (the stored/resolved type, since "no field focused" is a legitimate
// resting state, not a sentinel to special-case), so `form.focused_field`
// is lifted one level via `.map(Some)` before being passed in: `None`
// (uncontrolled) passes straight through, `Some(id)` (controlled) becomes
// `Some(Some(id))` — [`quadraui::WidgetStateStore::resolve`]'s own `Some(value)` arm then
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
fn resolve_form_focused_field(
    widget_state: &RefCell<quadraui::WidgetStateStore>,
    form: &Form,
) -> Option<WidgetId> {
    widget_state
        .borrow_mut()
        .resolve(form.id, form.focused_field.map(Some), || None)
}

/// Persists `new_field` as `form_id`'s own engine-owned focused field —
/// call whenever [`InteractionEngine::handle`]'s own form-focus routing
/// (Tab/Shift-Tab between fields, Up/Down between fields, or a field
/// click) decides on a new one. A later [`resolve_form_focused_field`]
/// call overwrites this the moment the matching `Form.focused_field` is
/// `Some` again — mirrors `host::apply_scroll_offset`'s own "the next
/// resolve decides whether this sticks" shape.
fn apply_form_focus_changed(
    widget_state: &RefCell<quadraui::WidgetStateStore>,
    form_id: WidgetId,
    new_field: WidgetId,
) {
    widget_state.borrow_mut().set(form_id, Some(new_field));
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
/// [`quadraui::compose::FormDialog`]'s
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

/// Whether `event` is a bare Escape press: the host's dismiss key for an
/// imperative overlay, which the overlay widgets themselves leave unclaimed.
fn is_escape(event: &UiEvent) -> bool {
    matches!(
        event,
        UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            ..
        }
    )
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

    /// A `"tree_table_controller"` extra's cached [`TreeTableController`]
    /// (`id` is the extra's own, not the event's node id) reported `event`
    /// — [`InteractionEngine::sync_tree_tables`]/[`InteractionEngine::handle`]'s
    /// own per-widget-id routing, mirroring the main `dispatch` above but
    /// carrying an already wire-shaped [`TreeTableControllerWireEvent`]
    /// rather than a [`quadraui::UiEvent`]: `quadraui::UiEvent` has no
    /// variant for this hierarchical, id-keyed controller yet (its
    /// flat-model `TreeTable`'s own `TreeTableEvent` carries a positional
    /// `idx`/`path`, and has no `ChildrenRequested`/ascending-aware
    /// `SortChanged` counterpart at all), so a binding wires this default
    /// method to its own callback however it threads a widget-scoped
    /// event today, once it exposes `"tree_table_controller"` over its own
    /// FFI surface.
    ///
    /// Defaults to ignoring the event and reporting
    /// [`quadraui::Reaction::Continue`] — every existing [`EventSink`] impl
    /// (a binding's own callback bridge, a `RecordingSink` test double)
    /// keeps compiling unchanged against this new default method until it
    /// deliberately overrides it.
    fn dispatch_tree_table_controller(
        &mut self,
        id: WidgetId,
        event: TreeTableControllerWireEvent,
    ) -> quadraui::Reaction {
        let _ = (id, event);
        quadraui::Reaction::Continue
    }

    /// [`InteractionEngine::handle`]'s own drag-gesture arm/continue/clear
    /// (see [`crate::drag::GestureDrag`]'s own doc) reported `event` —
    /// mirrors [`Self::dispatch_tree_table_controller`]'s own default-method
    /// rollout, carrying an already wire-shaped
    /// [`crate::semantic::DragGestureWireEvent`] rather than a
    /// [`quadraui::UiEvent`] for the identical reason that method does:
    /// `quadraui::UiEvent` has no variant for a generic draggable/
    /// drop-target gesture over an arbitrary leaf.
    ///
    /// Defaults to ignoring the event and reporting
    /// [`quadraui::Reaction::Continue`] — every existing [`EventSink`] impl
    /// keeps compiling unchanged against this new default method until it
    /// deliberately overrides it.
    fn dispatch_drag_gesture(&mut self, event: DragGestureWireEvent) -> quadraui::Reaction {
        let _ = event;
        quadraui::Reaction::Continue
    }

    /// A scroll input [`InteractionEngine::handle`] just resolved moved a
    /// windowed `list`/`tree`/`data_table`/`text_display` leaf's viewport
    /// outside the slice its app currently has loaded, so the app needs to
    /// re-render that leaf with `event`'s rows — mirrors
    /// [`Self::dispatch_tree_table_controller`]/[`Self::dispatch_drag_gesture`]'s
    /// own default-method rollout, carrying a
    /// [`crate::render::RowsRequestedEvent`] rather than a
    /// [`quadraui::UiEvent`] for the identical reason those two do:
    /// `quadraui::UiEvent` has no variant for a virtualised collection
    /// asking for more rows, so each binding surfaces this the way it
    /// surfaces any other synthesized event — see
    /// `docs/bindings-wire-format.md`'s `RowsRequestedEvent` entry.
    ///
    /// Only ever called for a leaf whose decode recorded an
    /// `"items_window"` (i.e. whose hit carries a
    /// [`crate::hit::LoadedRows`]), and never twice for the same request
    /// while the app's own loaded slice stays put, so an app that ignores
    /// one of these isn't re-asked every frame.
    ///
    /// Defaults to ignoring the event and reporting
    /// [`quadraui::Reaction::Continue`] — every existing [`EventSink`] impl
    /// keeps compiling unchanged against this new default method until it
    /// deliberately overrides it.
    fn dispatch_rows_requested(&mut self, event: RowsRequestedEvent) -> quadraui::Reaction {
        let _ = event;
        quadraui::Reaction::Continue
    }
}

/// Wire-shaped mirror of [`TreeTableControllerEvent`] — the same field
/// shapes, but `Serialize`/`Deserialize` (`TreeTableControllerEvent` itself
/// has neither; it's an in-crate-only `quadraui::compose` type), and
/// `Consumed`/`Ignored` dropped: [`InteractionEngine::handle`] only ever
/// calls [`EventSink::dispatch_tree_table_controller`] for the five
/// variants below, folding `Consumed`/`Ignored` straight into its own
/// `Reaction` instead (see that method's own `TreeTableController` routing
/// blocks).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum TreeTableControllerWireEvent {
    /// Row selected — click or keyboard navigation. `id` is the selected
    /// node's own [`quadraui::compose::NodeId`] (a plain `String`).
    RowSelected { id: String },
    /// Enter pressed, or a row/chevron double-clicked.
    RowActivated { id: String },
    /// A branch node's expand state flipped; its children were already
    /// loaded (or this collapses).
    ToggleExpand { id: String, expand: bool },
    /// A lazy branch was expanded for the first time with no loaded
    /// children — the host should fetch them and answer via
    /// [`InteractionEngine::set_tree_table_children`].
    ChildrenRequested { id: String },
    /// A header click (re-)sorted every loaded level's children by `col`'s
    /// cell text — see [`TreeTableController::header_clicked`]'s own doc
    /// for how a host overrides the comparator via
    /// [`InteractionEngine::set_tree_table_sort_override`].
    SortChanged { col: usize, ascending: bool },
}

impl TreeTableControllerWireEvent {
    /// Translates a [`TreeTableControllerEvent`] into its wire-shaped
    /// counterpart — `None` for `Consumed`/`Ignored`, which
    /// [`InteractionEngine::handle`]'s own callers fold into a `Reaction`
    /// directly rather than ever handing to [`EventSink::dispatch_tree_table_controller`].
    fn from_controller_event(event: TreeTableControllerEvent) -> Option<Self> {
        match event {
            TreeTableControllerEvent::RowSelected { id } => Some(Self::RowSelected { id }),
            TreeTableControllerEvent::RowActivated { id } => Some(Self::RowActivated { id }),
            TreeTableControllerEvent::ToggleExpand { id, expand } => {
                Some(Self::ToggleExpand { id, expand })
            }
            TreeTableControllerEvent::ChildrenRequested { id } => {
                Some(Self::ChildrenRequested { id })
            }
            TreeTableControllerEvent::SortChanged { col, ascending } => {
                Some(Self::SortChanged { col, ascending })
            }
            TreeTableControllerEvent::Consumed | TreeTableControllerEvent::Ignored => None,
        }
    }
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
    /// A contract widget shown imperatively — a message or form dialog, a
    /// completions list, a rich-text popup — painted through
    /// [`quadraui::DrawsPrimitives::draw_dyn`] and routed through its own
    /// [`DynWidget::handle`], with its state in
    /// [`InteractionEngine::widget_state`]. See [`WidgetOverlay`].
    Widget(WidgetOverlay),
    /// A modal [`Dialog`] hosting an arbitrary [`LayoutNode`] subtree in
    /// its body — opened via [`InteractionEngine::open_layout_dialog`].
    /// Shares [`Overlay::rank`] with the modal [`WidgetOverlay`] kinds, so
    /// only one modal dialog of any kind is ever open at once.
    LayoutDialog(Box<LayoutDialog>),
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
            Overlay::Widget(entry) => entry.kind.rank(),
            Overlay::LayoutDialog(_) => 4,
            Overlay::Tooltip(..) => 5,
            Overlay::Toast(_) => 6,
        }
    }
}

/// Which imperative surface an [`Overlay::Widget`] entry is: fixes its
/// z-rank, where it is laid out and how it claims input. The widget
/// itself is routed generically either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WidgetOverlayKind {
    /// [`MessageDialog`], opened via [`InteractionEngine::open_message_dialog`].
    MessageDialog,
    /// [`FormDialog`], opened via [`InteractionEngine::open_form_dialog`].
    FormDialog,
    /// [`Completions`], shown via [`InteractionEngine::show_completions`].
    Completions,
    /// [`RichTextPopup`], shown via [`InteractionEngine::show_rich_text_popup`].
    RichTextPopup,
}

impl WidgetOverlayKind {
    fn rank(self) -> u8 {
        match self {
            WidgetOverlayKind::RichTextPopup => 2,
            WidgetOverlayKind::Completions => 3,
            WidgetOverlayKind::MessageDialog | WidgetOverlayKind::FormDialog => 4,
        }
    }

    /// A modal dialog: laid out over the whole viewport (it centres itself
    /// there), routed every input event, and swallowing every press. A
    /// non-modal popup is laid out at its anchor, routed only the presses
    /// and wheel notches that land on it — every one of which it swallows
    /// — and dismissed by Escape or a press outside it.
    fn is_modal(self) -> bool {
        matches!(
            self,
            WidgetOverlayKind::MessageDialog | WidgetOverlayKind::FormDialog
        )
    }
}

/// One [`Overlay::Widget`] entry: the widget, which surface it is, and
/// the point a popup is anchored to (unused by a modal dialog).
struct WidgetOverlay {
    kind: WidgetOverlayKind,
    widget: Box<dyn DynWidget>,
    anchor: Point,
}

impl WidgetOverlay {
    fn new(kind: WidgetOverlayKind, widget: impl DynWidget, anchor: Point) -> Self {
        Self {
            kind,
            widget: Box::new(widget),
            anchor,
        }
    }

    /// The widget's own id — every overlay widget carries one.
    fn id(&self) -> WidgetId {
        self.widget
            .id()
            .copied()
            .unwrap_or_else(|| WidgetId::new("overlay"))
    }

    fn widget_as<T: 'static>(&self) -> Option<&T> {
        self.widget.as_any().downcast_ref::<T>()
    }

    fn widget_as_mut<T: 'static>(&mut self) -> Option<&mut T> {
        let widget: &mut dyn std::any::Any = &mut *self.widget;
        widget.downcast_mut::<T>()
    }

    /// Writes an event the widget reported back into its own descriptor
    /// where the surface keeps that value on the descriptor rather than in
    /// its interaction state: a completions list's selected row, a
    /// rich-text popup's scroll offset, a form dialog's focused field.
    /// Returns whether anything changed.
    fn apply_event(&mut self, event: &UiEvent) -> bool {
        match (self.kind, event) {
            (
                WidgetOverlayKind::Completions,
                UiEvent::List(_, ListViewEvent::SelectionChanged { idx }),
            ) => self
                .widget_as_mut::<Completions>()
                .map(|completions| completions.selected_idx = *idx)
                .is_some(),
            (WidgetOverlayKind::RichTextPopup, UiEvent::ScrollOffsetChanged { new_offset, .. }) => {
                self.widget_as_mut::<RichTextPopup>()
                    .map(|popup| popup.scroll_top = *new_offset)
                    .is_some()
            }
            (WidgetOverlayKind::FormDialog, UiEvent::Form(_, FormEvent::FocusChanged { id })) => {
                self.widget_as_mut::<FormDialog>()
                    .and_then(|dialog| dialog.form.as_mut())
                    .map(|form| form.focused_field = Some(*id))
                    .is_some()
            }
            _ => false,
        }
    }
}

/// The event [`InteractionEngine::dyn_widget_dispatch_with_focus`] hands
/// a widget: one its caller already routed, or a raw one to route against
/// the widget's own layout.
enum DispatchInput<'e> {
    Routed(RoutedEvent),
    Raw(&'e UiEvent),
}

/// How [`InteractionEngine::dyn_widget_dispatch_with_focus`] treats a
/// widget's focus: whether it is part of the backend's focus tree, and so
/// whether its `request_focus` moves real focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchFocus {
    /// A layout-tree leaf: focus is the backend's, and a focus request
    /// moves it.
    Tree,
    /// A popup floating over the focused editor: it never takes focus
    /// from it, so its focus request is dropped.
    Popup,
    /// A modal dialog: the dialog is the whole world while it is open, so
    /// every part of it holds focus.
    Modal,
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
/// own title/button chrome paints through the widget contract exactly as
/// for any other dialog; the subtree is painted into the
/// [`quadraui::DialogInput::ContentSlot`] hole the dialog reserves for it
/// (see [`Self::content_bounds`]).
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
/// same band [`FormDialog`] sizes itself in) and a third of the
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
    /// The chrome descriptor with the content subtree's slot filled in:
    /// a [`quadraui::DialogInput::ContentSlot`] carrying this dialog's
    /// requested width/height, so the widget contract reserves the hole
    /// the subtree paints into and sizes the box around it.
    fn paint_descriptor(&self) -> Dialog {
        Dialog {
            input: Some(quadraui::DialogInput::ContentSlot(
                quadraui::DialogContentSlot {
                    height: self.size.height,
                    width: self.size.width,
                },
            )),
            ..self.dialog.clone()
        }
    }

    /// The dialog's full layout for `backend`'s current viewport, with a
    /// body slot tall enough for the body text lines *plus* the content
    /// subtree. Resolved through the widget contract, the "one layout fn,
    /// two callers" pattern, so paint and hit-test can never disagree.
    fn layout(&self, backend: &dyn Backend) -> quadraui::DialogLayout {
        backend
            .layout_primitive(viewport_rect(backend), &self.paint_descriptor())
            .layout
    }

    /// The slot the content subtree paints into — the
    /// [`quadraui::DialogInput::ContentSlot`] hole
    /// [`Self::paint_descriptor`] reserves, which the dialog itself
    /// leaves blank.
    fn content_bounds(&self, layout: &quadraui::DialogLayout) -> Rect {
        layout.input_bounds.unwrap_or(layout.body_bounds)
    }

    /// Paints the dialog chrome and then the content subtree, caching
    /// this frame's button rects for [`InteractionEngine::tab_stops`].
    fn render(&self, backend: &mut dyn Backend) {
        let layout = self.layout(backend);
        backend.draw_dyn(viewport_rect(backend), &self.paint_descriptor(), &());
        let stops: Vec<(WidgetId, Rect)> = layout
            .visible_buttons
            .iter()
            .map(|b| (b.id, b.bounds))
            .collect();
        let content_area = self.content_bounds(&layout);
        self.content.render(backend, content_area, &stops);
        *self.button_stops.borrow_mut() = stops;
    }

    /// [`quadraui::Dialog::cancel_button_id`], falling back to
    /// [`quadraui::Dialog::default_button_id`] and then to the last
    /// button — Escape's "there's always something to resolve to"
    /// fallback, the same order [`FormDialog`] uses.
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
    let focused = backend.focus_tree().focused_id().copied();
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
                let focused = backend.focus_tree().focused_id().copied();
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
/// The in-window menu behind a `"menu_bar"` extra, with its
/// [`AppMenuState`].
struct MenuBarHost {
    menu: AppMenu,
    state: AppMenuState,
}

impl Default for MenuBarHost {
    fn default() -> Self {
        Self {
            menu: AppMenu::new(WidgetId::new("menu-system-bar"), Vec::new()),
            state: AppMenuState::default(),
        }
    }
}

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
    /// This frame's draggable/drop-target roles, keyed by the same
    /// [`WidgetId`] space [`Self::hit_map`]'s own entries carry —
    /// [`crate::hit::drag_roles_by_widget_id`]'s `WidgetId`-keyed
    /// reshaping of [`crate::wire::DecodeDiagnostics::drag_roles`]'s own
    /// decode-time, `String`-keyed side table. [`Self::set_drag_roles`]
    /// replaces this wholesale, mirroring [`Self::set_hits`]'s own
    /// per-frame lifecycle for [`Self::hit_map`] — unlike
    /// `DecodeDiagnostics::drag_roles`, which accumulates for its whole
    /// session and is never scoped to "painted this frame", this is
    /// cleared (via a `HashMap::new()` call) the moment a binding stops
    /// calling `set_drag_roles` for a given render. Empty until a binding
    /// starts calling [`Self::set_drag_roles`]; no gesture logic reads it
    /// yet — this is the seam a future drag/drop dispatch reads from.
    drag_roles: RefCell<HashMap<WidgetId, DragRole>>,
    /// The last rows a windowed leaf's app was asked for through
    /// [`EventSink::dispatch_rows_requested`], keyed by that leaf's id:
    /// `(the loaded slice the request was computed against, start, count)`.
    ///
    /// [`Self::dispatch_rows_requested`] suppresses a request identical to
    /// the stored one, so an app that doesn't answer (or hasn't answered
    /// yet) isn't re-asked on every subsequent scroll event. The loaded
    /// slice is part of the key, not just the request: the moment the app
    /// does answer, the leaf's own [`crate::hit::LoadedRows`] changes and
    /// the next genuinely-uncovered scroll asks again rather than being
    /// mistaken for a repeat.
    rows_requested: RefCell<HashMap<WidgetId, (LoadedRows, usize, usize)>>,
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
    /// Armed by a `MouseDown` landing on a leaf [`Self::drag_roles`] marks
    /// [`DragRole::draggable`], and cleared on the matching `MouseUp` —
    /// see [`crate::drag::GestureDrag`]'s own doc for how this differs
    /// from [`Self::active_drag`]/[`Self::scrollbar_drag`]/
    /// [`Self::column_drag`]/[`Self::scroll_drag`] above: those four
    /// rewrite the raw `MouseMoved`/`MouseUp` into a replacement
    /// `UiEvent`; this one leaves them untouched and instead dispatches
    /// [`crate::semantic::DragGestureWireEvent::DragStarted`]/`DragOver`/
    /// `Dropped` alongside whatever else the same events already produce,
    /// through [`EventSink::dispatch_drag_gesture`].
    gesture_drag: RefCell<Option<GestureDrag>>,
    /// Per-`TextInput` [`quadraui::TextEditor`] session, keyed by
    /// `WidgetId`, that survives across keystrokes — see
    /// [`crate::text_input::apply_text_input_edit`]'s own doc for why a
    /// cache is needed at all, including its own `undo_stack`.
    text_editors: TextEditorCache,
    /// The one per-widget state store, keyed by `WidgetId` and surviving
    /// across frames ([`quadraui::WidgetStateStore`]): every
    /// [`crate::layout::LeafDescriptor::Widget`] leaf's own
    /// [`DynWidget`] state, and the typed values the binding host resolves
    /// for containers and the fixed leaf kinds — a `Tree`'s cursor, a
    /// `Scroll`'s offset, a split's ratio, a tab strip's active index, a
    /// tree's expansion. [`Self::handle`] writes those values through
    /// [`crate::host`]'s `apply_*` helpers; a binding's own `render` reads
    /// them back (merged with whatever the descriptor pins) through its
    /// `resolve_*` helpers. [`Self::set_hits`] runs the store's frame
    /// sweep every frame.
    widget_state: RefCell<quadraui::WidgetStateStore>,
    /// Owns the in-window dropdown behind a `"menu_bar"` extra: the
    /// open/close/keyboard-nav/submenu state machine that turns a click on
    /// a top-level item into an open dropdown and a selection into
    /// `UiEvent::MenuActivated`.
    menu_system: RefCell<MenuBarHost>,
    /// The absolute bounds of the `"menu_bar"` extra [`InteractionEngine::sync_menu_bar`]
    /// most recently painted, read by [`InteractionEngine::handle`] to call
    /// [`quadraui::AppMenu::handle_ui_event`] with the same `bar_rect` the
    /// paint pass used.
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
    /// The whole-widget-level hover target — any painted primitive — the
    /// most recent `MouseMoved` landed on, paired
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
    /// recently observed as `backend.focus_tree().focused_id()` itself —
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
    /// One [`TreeTableController`] per `"tree_table_controller"` extra
    /// painted this (or a prior) frame, keyed by the extra's own id —
    /// generalizes [`Self::form_controllers`]'s "one stateful controller
    /// per widget id, rebuilt every render" pattern to this hierarchical
    /// model. [`Self::sync_tree_tables`] seeds a newly-created entry from
    /// the extra's own `nodes`/`selected_id` exactly once
    /// ([`crate::extra_paint::TreeTableControllerDescriptor::seed_controller`]);
    /// from then on the controller owns its own expand/selection/sort
    /// state, same as [`Self::form_controllers`] owns `scroll_offset` from
    /// its own creation onward.
    tree_table_controllers: RefCell<HashMap<WidgetId, TreeTableController>>,
    /// The absolute bounds and current `columns` [`Self::sync_tree_tables`]
    /// most recently painted each [`Self::tree_table_controllers`] entry
    /// with — [`TreeTableController::handle`] takes both as plain
    /// arguments rather than storing them (see that method's own doc), so
    /// [`Self::handle`]'s routing needs both back to call it.
    tree_table_rects: RefCell<HashMap<WidgetId, (Rect, Vec<Column>)>>,
    /// This frame's `"tree_table_controller"` extra ids, in the paint
    /// order [`Self::sync_tree_tables`] saw them — mirrors
    /// [`Self::form_order`]'s own doc for why a `MouseDown` claim loop
    /// walks this rather than the `HashMap`s above directly.
    tree_table_order: RefCell<Vec<WidgetId>>,
    /// The id of the [`PaintedHit::Widget`] leaf currently holding a
    /// [`quadraui::interactive::EventHost::capture_pointer`] request, if
    /// any — the generic [`crate::extra_paint::WIDGET_KINDS`] seam's own
    /// counterpart of [`Self::active_drag`]/[`Self::scrollbar_drag`]/
    /// [`Self::column_drag`]/[`Self::scroll_drag`]. Set by
    /// [`Self::dyn_widget_dispatch`] when a `DynWidget::handle` call
    /// requests capture (e.g. [`CellGrid`]'s own drag-to-extend-selection);
    /// cleared the same way on a release request, and unconditionally on
    /// every `MouseUp` — mirroring those four fields' own "`MouseUp`
    /// always ends it" reset.
    dyn_widget_capture: RefCell<Option<WidgetId>>,
    /// The [`crate::extra_paint::WIDGET_KINDS`] leaf the pointer was last
    /// over, so a move that leaves it routes a `Leave` there and one that
    /// arrives on another routes an `Enter` (see
    /// [`Self::route_dyn_widget_hover`]).
    dyn_widget_hover: RefCell<Option<WidgetId>>,
    /// The sub-target a `DynWidget` leaf last asked focus for through
    /// [`quadraui::interactive::EventHost::request_focus`], with the leaf's
    /// id. `Backend::set_focus` addresses a whole widget, so the sub-part
    /// lives here; it answers [`EventHost::is_focused`] only while the
    /// backend's focus is still on that same leaf.
    dyn_widget_focus_sub: RefCell<Option<quadraui::FocusAddress>>,
}

/// What [`InteractionEngine::dispatch_dyn_widget_click`] resolved —
/// mirrors the `Option<UiEvent>` / `attach_hit` pair its caller already
/// threads through every other `detail` arm, bundled with the `Reaction`s
/// produced along the way (an `EventCx::emit`ted event's own dispatch, or
/// the bare `Redraw` [`quadraui::Handled::Consumed`] asks for) so the
/// caller folds exactly one value into its own running `Reaction`, same
/// as [`InteractionEngine::scroll_focused_child_into_view`]'s return.
struct DynWidgetClickOutcome {
    /// Replacement semantic event, when [`DynWidget::handle`] resolved one
    /// ([`quadraui::Handled::Event`]) — the caller swaps it in for the raw
    /// `MouseDown`/`DoubleClick` and stops attaching the raw hit, mirroring
    /// every other semantic substitution in [`InteractionEngine::handle`].
    event: Option<UiEvent>,
    /// `true` for [`quadraui::Handled::Consumed`]: the click changed the
    /// widget's own state with no event to route, so the caller still
    /// swallows the raw click (same as `event.is_some()`) but has nothing
    /// to swap `event` for.
    consumed: bool,
    /// Every `Reaction` already dispatched/resolved by this call.
    reaction: quadraui::Reaction,
}

/// The pointer-click facts [`InteractionEngine::handle`] has already
/// resolved by the time a hit reaches
/// [`InteractionEngine::dispatch_dyn_widget_click`] — bundled into one
/// value so that method stays under `clippy::too_many_arguments` without
/// losing any of them.
struct PointerClick {
    position: Point,
    button: MouseButton,
    modifiers: Modifiers,
    double_click: bool,
}

impl Default for DynWidgetClickOutcome {
    fn default() -> Self {
        Self {
            event: None,
            consumed: false,
            reaction: quadraui::Reaction::Continue,
        }
    }
}

/// [`EventCx::emit`]/[`EventCx::request_focus`]/[`EventCx::request_redraw`]'s
/// outbox for one [`DynWidget::handle`] call — mirrors `quadraui::runtime`'s
/// own private `WidgetHostIntents`/`WidgetEventHost` pair (the reference
/// impl backing the same trait for the widget-contract routing engine):
/// recording what the widget asked for here, rather than reaching for
/// `backend`/`sink` directly from inside [`DynWidgetEventHost`]'s own
/// methods, sidesteps a second concurrent borrow of either while
/// [`DynWidget::handle`] itself still holds `&mut dyn Any` into
/// [`InteractionEngine::dyn_widget_state`].
#[derive(Default)]
struct DynWidgetIntents {
    focus_request: Option<quadraui::FocusAddress>,
    redraw: bool,
    emitted: Vec<UiEvent>,
    /// Set by [`EventHost::capture_pointer`] — [`InteractionEngine::dyn_widget_dispatch`]
    /// applies this onto [`InteractionEngine::dyn_widget_capture`].
    capture_requested: bool,
    /// Set by [`EventHost::release_pointer`] — same application site as
    /// [`Self::capture_requested`].
    release_requested: bool,
}

/// [`EventHost`] impl backing one [`DynWidget::handle`] call, scoped to a
/// `LeafDescriptor::Widget` leaf's event dispatch. `set_cursor` is a
/// no-op — not wired yet rather than impossible: this engine has no
/// generic per-widget cursor-shape tracking outside its own bespoke drag
/// state machines (`Self::active_drag` and friends), and no
/// `WIDGET_KINDS` member calls it yet. `capture_pointer`/`release_pointer`
/// record an intent rather than touching [`InteractionEngine::dyn_widget_capture`]
/// directly, same reason `request_focus` records one instead of calling
/// `backend.set_focus` — this host has no `&mut InteractionEngine` to
/// reach either.
struct DynWidgetEventHost<'a> {
    units: quadraui::primitive::Units,
    /// The focused leaf and sub-part when this call began — see
    /// [`InteractionEngine::dyn_widget_focus`].
    focused: Option<quadraui::FocusAddress>,
    /// Every part of the widget holds focus — a modal dialog, which is
    /// the whole world while it is open (see [`DispatchFocus::Modal`]).
    whole_world: bool,
    intents: &'a mut DynWidgetIntents,
}

impl EventHost for DynWidgetEventHost<'_> {
    fn is_focused(&self, id: &WidgetId, sub: Option<quadraui::interactive::SubTarget>) -> bool {
        self.whole_world || self.focused == Some((*id, sub))
    }

    fn request_focus(&mut self, id: &WidgetId, sub: Option<quadraui::interactive::SubTarget>) {
        self.intents.focus_request = Some((*id, sub));
    }

    fn capture_pointer(&mut self, _id: &WidgetId) {
        self.intents.capture_requested = true;
    }

    fn release_pointer(&mut self) {
        self.intents.release_requested = true;
    }

    fn set_cursor(&mut self, _cursor: PointerShape) {}

    fn request_redraw(&mut self) {
        self.intents.redraw = true;
    }

    fn emit(&mut self, event: UiEvent) {
        self.intents.emitted.push(event);
    }

    fn units(&self) -> quadraui::primitive::Units {
        self.units
    }
}

/// The `(area, widget)` of the [`PaintedHit::Widget`] painted under `id`
/// this frame, or `None` if no such hit exists (including a stale `id`
/// from a previous frame's hit map). `widget` is an `Rc` clone, same
/// reason [`PaintedHit::Widget`]'s own field is one — see that field's
/// doc.
fn dyn_widget_hit(hit_map: &[PaintedHit], id: WidgetId) -> Option<(Rect, Rc<dyn DynWidget>)> {
    hit_map.iter().find_map(|hit| match hit {
        PaintedHit::Widget {
            id: hit_id,
            area,
            widget,
        } if *hit_id == id => Some((*area, Rc::clone(widget))),
        _ => None,
    })
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

    /// This frame's draggable/drop-target roles, keyed by [`WidgetId`] —
    /// the [`Self::drag_roles`] field's own read access, mirroring
    /// [`Self::hit_map`]'s. `None` entries mean "this id has no recorded
    /// role" rather than appearing with [`DragRole::default`] — a leaf
    /// never seen by [`Self::set_drag_roles`] (including every binding
    /// that never calls it at all) simply has no key here.
    pub fn drag_roles(&self) -> Ref<'_, HashMap<WidgetId, DragRole>> {
        self.drag_roles.borrow()
    }

    /// Replaces this frame's retained draggable/drop-target roles,
    /// keyed by [`WidgetId`] (see [`crate::hit::drag_roles_by_widget_id`]
    /// for building one from [`crate::wire::DecodeDiagnostics::drag_roles`]'s
    /// own decode-time, `String`-keyed table). Call once per render,
    /// alongside [`Self::set_hits`] — this is a sibling call, not folded
    /// into `set_hits` itself, so a binding that never decodes a drag
    /// role (the common case today) keeps calling `set_hits` exactly as
    /// before. No gesture logic reads [`Self::drag_roles`] yet.
    pub fn set_drag_roles(&self, drag_roles: HashMap<WidgetId, DragRole>) {
        *self.drag_roles.borrow_mut() = drag_roles;
    }

    /// This engine's own per-widget state store — a binding's `render`
    /// passes this to [`crate::host::resolve_tree_cursor`] (right before
    /// painting a `Tree`) so the cursor [`Self::handle`]'s `KeyPressed`
    /// routing moved via [`crate::host::apply_tree_nav_key`] is what
    /// actually gets painted.
    pub fn widget_state(&self) -> &RefCell<quadraui::WidgetStateStore> {
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
    /// Also runs [`Self::widget_state`]'s frame sweep
    /// ([`quadraui::WidgetStateStore::gc`]): every generic widget painted
    /// this frame and every open overlay keeps its state, as does every
    /// value the binding host resolved this frame; everything else is
    /// dropped.
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
        // Same "painted this frame" retain for the windowed-leaf request
        // bookkeeping: a leaf that stops rendering (or stops sending an
        // `items_window`) must not leave its last request behind to
        // suppress a future one under a reused id.
        self.rows_requested
            .borrow_mut()
            .retain(|id, _| windowed_leaf_extent(&hits, *id).is_some());
        {
            let mut store = self.widget_state.borrow_mut();
            for hit in &hits {
                if let PaintedHit::Widget { id, .. } = hit {
                    store.mark_seen(id);
                }
            }
            for id in self.overlay_widget_ids() {
                store.mark_seen(&id);
            }
            store.gc();
        }
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
            glyphs: Default::default(),
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
        self.widget_overlay(WidgetOverlayKind::MessageDialog)
            .is_some()
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
        if self.menu_system.borrow().menu.is_open() {
            self.menu_system.borrow_mut().menu.dismiss(backend);
        }
        if self.context_menu_open() {
            self.close_context_menu(backend);
        }
        let dialog = MessageDialog::new(opts);
        let bounds = dialog.bounds(backend);
        backend
            .modal_stack_handle()
            .borrow_mut()
            .push(dialog.dialog.id, bounds);
        self.widget_state.borrow_mut().remove(&dialog.dialog.id);
        self.push_overlay(Overlay::Widget(WidgetOverlay::new(
            WidgetOverlayKind::MessageDialog,
            dialog,
            Point::default(),
        )));
    }

    /// Closes the open message dialog (if any), popping its
    /// [`quadraui::ModalStack`] entry. Returns whether a dialog was open to
    /// close. Exposed so a binding's own `show_message_dialog` surface can
    /// offer a programmatic dismiss alongside input-driven resolution
    /// (Enter/Escape/click) — without this, a dialog with no buttons at
    /// all, or one the app wants to tear down from outside input, could
    /// never be cleared. A no-op (returns `false`) if nothing is open.
    pub fn close_message_dialog(&mut self, backend: &mut dyn Backend) -> bool {
        self.close_modal_widget_overlay(WidgetOverlayKind::MessageDialog, backend)
    }

    /// Removes the open modal widget overlay of `kind`, popping its
    /// [`quadraui::ModalStack`] entry and dropping its state. Returns
    /// whether one was open.
    fn close_modal_widget_overlay(
        &mut self,
        kind: WidgetOverlayKind,
        backend: &mut dyn Backend,
    ) -> bool {
        match self.remove_overlay(|o| matches!(o, Overlay::Widget(entry) if entry.kind == kind)) {
            Some(Overlay::Widget(open)) => {
                let id = open.id();
                backend.modal_stack_handle().borrow_mut().pop(&id);
                self.widget_state.borrow_mut().remove(&id);
                true
            }
            _ => false,
        }
    }

    /// The open widget overlay of `kind`, if any.
    fn widget_overlay(&self, kind: WidgetOverlayKind) -> Option<&WidgetOverlay> {
        self.overlays.iter().find_map(|o| match o {
            Overlay::Widget(entry) if entry.kind == kind => Some(entry),
            _ => None,
        })
    }

    fn widget_overlay_mut(&mut self, kind: WidgetOverlayKind) -> Option<&mut WidgetOverlay> {
        self.overlays.iter_mut().find_map(|o| match o {
            Overlay::Widget(entry) if entry.kind == kind => Some(entry),
            _ => None,
        })
    }

    /// The ids of every open widget overlay, whose state
    /// [`Self::set_hits`]'s frame sweep keeps while they stay open.
    fn overlay_widget_ids(&self) -> Vec<WidgetId> {
        self.overlays
            .iter()
            .filter_map(|o| match o {
                Overlay::Widget(entry) => Some(entry.id()),
                _ => None,
            })
            .collect()
    }

    /// Whether a form-hosting dialog opened via [`Self::open_form_dialog`]
    /// is currently open.
    pub fn form_dialog_open(&self) -> bool {
        self.widget_overlay(WidgetOverlayKind::FormDialog).is_some()
    }

    /// Opens `dialog` — a [`FormDialog`] hosting a
    /// [`quadraui::Form`]'s fields inside a modal [`quadraui::Dialog`]'s own
    /// title/button chrome — pushing its bounds onto the backend's
    /// [`quadraui::ModalStack`], the same contract
    /// [`Self::open_message_dialog`] already follows. Replaces (and pops)
    /// any modal dialog already open, of any of the three kinds: they all
    /// share [`Overlay::rank`], so [`Self::push_overlay`]'s "same rank,
    /// same slot" rule keeps "only one modal dialog at a time" true
    /// across all of them. A binding's own `show_form_dialog` surface calls this, then
    /// pushes the app's current field values onto `dialog` every frame
    /// via [`Self::form_dialog_mut`] before
    /// [`Self::render_overlays`] paints it — the same "app owns the
    /// data, the widget's state owns the edits" contract a bare
    /// [`FormController`] consumer already follows.
    pub fn open_form_dialog(&mut self, dialog: FormDialog, backend: &mut dyn Backend) {
        self.close_form_dialog(backend);
        self.close_message_dialog(backend);
        self.close_layout_dialog(backend);
        if self.menu_system.borrow().menu.is_open() {
            self.menu_system.borrow_mut().menu.dismiss(backend);
        }
        if self.context_menu_open() {
            self.close_context_menu(backend);
        }
        let bounds = dialog.bounds(&FormDialogState::default(), backend);
        backend
            .modal_stack_handle()
            .borrow_mut()
            .push(dialog.id, bounds);
        self.widget_state.borrow_mut().remove(&dialog.id);
        self.push_overlay(Overlay::Widget(WidgetOverlay::new(
            WidgetOverlayKind::FormDialog,
            dialog,
            Point::default(),
        )));
    }

    /// Closes the open form-dialog (if any), popping its
    /// [`quadraui::ModalStack`] entry. Returns whether one was open.
    /// Mirrors [`Self::close_message_dialog`]'s own doc.
    pub fn close_form_dialog(&mut self, backend: &mut dyn Backend) -> bool {
        self.close_modal_widget_overlay(WidgetOverlayKind::FormDialog, backend)
    }

    /// Mutable access to the open form-dialog widget, if any — a
    /// binding's render path uses this to set its [`FormDialog::form`] and
    /// [`FormDialog::body`] every frame before painting, the same "push
    /// fresh state, then render" contract a bare [`FormController`]
    /// consumer already follows.
    pub fn form_dialog_mut(&mut self) -> Option<&mut FormDialog> {
        self.widget_overlay_mut(WidgetOverlayKind::FormDialog)?
            .widget_as_mut::<FormDialog>()
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
    /// [`MessageDialog::dialog_layout`] already follows for `Dialog`.
    fn toast_layout(&self, backend: &dyn Backend) -> ToastStackLayout {
        backend
            .layout_primitive(viewport_rect(backend), &self.toast_overlay())
            .layout
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

    /// Places `tooltip` against `anchor`, sized by the tooltip's own
    /// [`quadraui::primitive::Primitive::measure`].
    fn tooltip_layout(
        &self,
        backend: &dyn Backend,
        tooltip: &Tooltip,
        anchor: Rect,
    ) -> TooltipLayout {
        let line_height = backend.line_height();
        let ctx = quadraui::primitive::LayoutCtx::new(
            line_height,
            backend.char_width(),
            backend.nerd_fonts_enabled(),
        );
        let natural = quadraui::primitive::Primitive::measure(
            tooltip,
            &ctx,
            quadraui::primitive::Size::new(0.0, 0.0),
        );
        let measure =
            TooltipMeasure::new(natural.width.unwrap_or(0.0), natural.height.unwrap_or(0.0));
        tooltip.layout(anchor, viewport_rect(backend), measure, line_height)
    }

    // ── Completions ──────────────────────────────────────────────────

    /// Shows `completions` anchored at `cursor` (the caret position in
    /// the backend's ABSOLUTE frame). Replaces any popup already open.
    pub fn show_completions(&mut self, completions: Completions, cursor: Point) {
        self.push_overlay(Overlay::Widget(WidgetOverlay::new(
            WidgetOverlayKind::Completions,
            completions,
            cursor,
        )));
    }

    /// Closes the open completions popup (if any) without reporting a
    /// selection. Returns whether one was open.
    pub fn dismiss_completions(&mut self) -> bool {
        self.remove_widget_overlay(WidgetOverlayKind::Completions)
    }

    /// The open completions popup (if any) and the cursor position it was
    /// anchored to.
    #[cfg(test)]
    fn find_completions(&self) -> Option<(Completions, Point)> {
        let entry = self.widget_overlay(WidgetOverlayKind::Completions)?;
        Some((entry.widget_as::<Completions>()?.clone(), entry.anchor))
    }

    /// Removes the open widget overlay of `kind` and drops its state.
    /// Returns whether one was open.
    fn remove_widget_overlay(&mut self, kind: WidgetOverlayKind) -> bool {
        match self.remove_overlay(|o| matches!(o, Overlay::Widget(entry) if entry.kind == kind)) {
            Some(Overlay::Widget(open)) => {
                self.widget_state.borrow_mut().remove(&open.id());
                true
            }
            _ => false,
        }
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
    /// wrapped in `Row`/`Column`/`Split`/`Tabs`/`SidebarPanel`/
    /// `SplitTree`/`Scroll`/`Styled`) receives focus, click and keyboard
    /// routing exactly as the same leaf would in the main tree: a click
    /// on a `Button` emits the normal
    /// [`quadraui::UiEvent::ButtonActivated`]; a click or focused key
    /// on a `Tree`/`List`/`DataTable`/`TabBar`/`TextDisplay`/`Checkbox`/…
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
        if self.menu_system.borrow().menu.is_open() {
            self.menu_system.borrow_mut().menu.dismiss(backend);
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
        self.push_overlay(Overlay::LayoutDialog(Box::new(embedded)));
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
        self.push_overlay(Overlay::Widget(WidgetOverlay::new(
            WidgetOverlayKind::RichTextPopup,
            popup,
            anchor,
        )));
    }

    /// Closes the open rich-text popup (if any) without reporting a link
    /// click. Returns whether one was open.
    pub fn dismiss_rich_text_popup(&mut self) -> bool {
        self.remove_widget_overlay(WidgetOverlayKind::RichTextPopup)
    }

    /// The open rich-text popup (if any) and the anchor it was shown
    /// against.
    #[cfg(test)]
    fn find_rich_text_popup(&self) -> Option<(RichTextPopup, Point)> {
        let entry = self.widget_overlay(WidgetOverlayKind::RichTextPopup)?;
        Some((entry.widget_as::<RichTextPopup>()?.clone(), entry.anchor))
    }

    /// Where `entry` is laid out this frame: the whole viewport for a
    /// modal dialog, which centres itself there, and the box its anchor
    /// resolves to for a popup.
    fn widget_overlay_bounds(&self, backend: &dyn Backend, entry: &WidgetOverlay) -> Rect {
        let anchored = match entry.kind {
            WidgetOverlayKind::MessageDialog | WidgetOverlayKind::FormDialog => None,
            WidgetOverlayKind::Completions => entry
                .widget_as::<Completions>()
                .map(|c| self.completions_layout(backend, c, entry.anchor).bounds),
            WidgetOverlayKind::RichTextPopup => entry
                .widget_as::<RichTextPopup>()
                .map(|p| self.rich_text_popup_layout(backend, p, entry.anchor).bounds),
        };
        anchored.unwrap_or_else(|| viewport_rect(backend))
    }

    /// Paints `entry` into `rect` with the state [`Self::widget_state`]
    /// holds for it: the state is lent to this thread's runtime store for
    /// the one `draw_dyn` call, which is where a widget's paint reads its
    /// own state from ([`quadraui::PaintEnv::widget_state`]).
    fn paint_widget_overlay(&self, backend: &mut dyn Backend, rect: Rect, entry: &WidgetOverlay) {
        let id = entry.id();
        let lent = self.widget_state.borrow_mut().take(&id);
        if let Some(state) = lent {
            quadraui::with_widget_states(|states| states.insert(id, state));
        }
        backend.draw_dyn(rect, entry.widget.as_ref(), &());
        let returned = quadraui::with_widget_states(|states| states.take(&id));
        if let Some(state) = returned {
            self.widget_state.borrow_mut().insert(id, state);
        }
    }

    fn rich_text_popup_layout(
        &self,
        backend: &dyn Backend,
        popup: &RichTextPopup,
        anchor: Point,
    ) -> RichTextPopupLayout {
        let m = backend.measure();
        let measure = popup.measure_for(backend, m.char_width * 30.0);
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
                    backend.draw_dyn(layout.bounds, float, &());
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
                Overlay::Widget(entry) => {
                    let rect = self.widget_overlay_bounds(backend, entry);
                    self.paint_widget_overlay(backend, rect, entry);
                }
                Overlay::LayoutDialog(embedded) => embedded.render(backend),
                Overlay::Tooltip(tooltip, anchor) => {
                    let layout = self.tooltip_layout(backend, tooltip, *anchor);
                    backend.draw_dyn(layout.bounds, tooltip, &());
                }
                // A toast stack paints as one group, after this loop —
                // see the call below.
                Overlay::Toast(_) => {}
            }
        }
        if self.overlays.iter().any(|o| matches!(o, Overlay::Toast(_))) {
            let overlay = self.toast_overlay();
            backend.draw_dyn(viewport_rect(backend), &overlay, &());
        }
    }

    /// Derives this frame's tab stops directly from the hit map
    /// [`InteractionEngine::set_hits`] last recorded, in paint order.
    /// `Split`/`SplitTree` are chrome around other content, not focusable
    /// leaves themselves, so they are skipped, and a `SidebarPanel`'s
    /// pointer-only header contributes none; `Extra` is a tab stop only when its
    /// descriptor opts in via
    /// [`crate::extra_paint::ExtraPaint::tab_stop`] (currently
    /// `TextInput`); `TextDisplay` is always one (scroll/copy need
    /// keyboard focus the same way a `Tree`/`List` does); every other
    /// variant yields its id and bounding `Rect`.
    ///
    /// Returns empty while an [`Overlay::FormDialog`] or a Form-content
    /// [`Overlay::Float`] (a popover opened via [`Self::show_popover`]) is
    /// open, regardless of what the main tree itself painted this frame —
    /// both trap Tab inside their own embedded form internally (see
    /// [`FormDialog`]'s "Focus model" doc section and
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
                Overlay::Widget(WidgetOverlay {
                    kind: WidgetOverlayKind::FormDialog,
                    ..
                }) | Overlay::Float(_, _, Some(FloatContent::Form(_)))
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
    /// `menu_system.menu.render` draws from that `AppMenu`'s own
    /// open/focused/id state (see `AppMenu::build_menu_bar`), not the descriptor's
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
            // Close any dropdown first so the dropdown's entry
            // `AppMenu::handle_ui_event` pushed onto the modal stack
            // gets popped — otherwise it leaks, and the dropdown would
            // silently reappear (with nothing to hit-test it) if the
            // extra ever comes back.
            if self.menu_system.borrow().menu.is_open() {
                self.menu_system.borrow_mut().menu.dismiss(backend);
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
        menu_system.menu.set_menus(menus);
        menu_system.menu.render(backend, area);
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
            let engine_focused = backend.focus_tree().focused_id() == Some(id);
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

    /// The [`crate::extra_paint::TreeTableControllerDescriptor`]
    /// counterpart of [`Self::sync_forms`]: every `"tree_table_controller"`
    /// extra painted this frame gets its own cached
    /// [`TreeTableController`] (creating and seeding one the first time an
    /// id is seen, via [`crate::extra_paint::TreeTableControllerDescriptor::seed_controller`]),
    /// repainted from that controller's own live expand/selection/sort
    /// state instead of the plain, stateless [`crate::extra_paint::ExtraPaint::paint`]
    /// every extra gets by default. Call once per render, after the extras
    /// loop in `draw_bands`/`paint_leaf` already painted the plain
    /// fallback.
    ///
    /// `has_focus`/`show_scrollbar`/`bordered` ARE reconciled every frame
    /// (same as [`Self::sync_forms`]'s own `has_focus`): they're cosmetic,
    /// host-echoed flags with no internal state of the controller's own to
    /// clobber. `nodes`/`selected_id` are not — see [`Self::tree_table_controllers`]'s
    /// own doc for why those are seeded once, on creation, and never again.
    ///
    /// A cached entry whose id wasn't painted this frame is dropped, so a
    /// tree table that stops rendering doesn't keep answering events
    /// forever.
    pub fn sync_tree_tables(&self, backend: &mut dyn Backend, hits: &[PaintedHit]) {
        let mut seen = Vec::new();
        for hit in hits {
            let PaintedHit::Extra {
                id, area, extra, ..
            } = hit
            else {
                continue;
            };
            let Some(descriptor) = extra_as::<TreeTableControllerDescriptor>(extra) else {
                continue;
            };
            seen.push(*id);
            let engine_focused = backend.focus_tree().focused_id() == Some(id);
            let mut controllers = self.tree_table_controllers.borrow_mut();
            let controller = controllers
                .entry(*id)
                .or_insert_with(|| descriptor.seed_controller());
            controller.set_has_focus(descriptor.has_focus || engine_focused);
            controller.set_show_scrollbar(descriptor.show_scrollbar);
            controller.set_bordered(descriptor.bordered);
            controller.render(backend, *area, descriptor.columns.clone());
            self.tree_table_rects
                .borrow_mut()
                .insert(*id, (*area, descriptor.columns.clone()));
        }
        self.tree_table_controllers
            .borrow_mut()
            .retain(|id, _| seen.contains(id));
        self.tree_table_rects
            .borrow_mut()
            .retain(|id, _| seen.contains(id));
        *self.tree_table_order.borrow_mut() = seen;
    }

    /// Answers a [`TreeTableControllerWireEvent::ChildrenRequested`] for
    /// the `"tree_table_controller"` cached under `id`: attaches
    /// `children` to `node_id` via [`TreeTableController::set_children`]
    /// (found anywhere in the tree, not just at the top level — see that
    /// method's own doc) and clears it from the controller's own pending-
    /// request set. A no-op if `id` names no cached controller (it hasn't
    /// been painted this frame, or at all yet).
    pub fn set_tree_table_children(
        &self,
        id: WidgetId,
        node_id: &str,
        children: Vec<TreeTableNodeData>,
    ) {
        if let Some(controller) = self.tree_table_controllers.borrow_mut().get_mut(&id) {
            controller.set_children(&node_id.to_string(), children);
        }
    }

    /// Overrides the comparator a [`TreeTableControllerWireEvent::SortChanged`]
    /// this engine already emitted applied by default: replaces every
    /// loaded node under `id`'s cached controller with `nodes`, already
    /// ordered however the host likes (numeric, locale-aware, ...) — a
    /// thin [`TreeTableController::set_nodes`] forward, named for what a
    /// binding's own sort-override call does with it. A no-op if `id`
    /// names no cached controller.
    pub fn set_tree_table_sort_override(&self, id: WidgetId, nodes: Vec<TreeTableNodeData>) {
        if let Some(controller) = self.tree_table_controllers.borrow_mut().get_mut(&id) {
            controller.set_nodes(nodes);
        }
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
    /// [`quadraui::compose::FormDialog`]'s
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
            backend.draw_dyn(*area, &repainted, &());
        }
    }

    /// Translates a [`TreeTableController::handle`] outcome into the
    /// `Reaction` [`Self::handle`]'s own `"tree_table_controller"` routing
    /// blocks return: `None` for [`TreeTableControllerEvent::Ignored`] —
    /// nothing claimed the event, so the caller's own fallback keeps
    /// running, mirroring every `Handled::Ignored` check the `"form"`
    /// blocks already make. `Some(Reaction::Redraw)` for `Consumed`;
    /// `Some(reaction)` for each of the five real variants, folding in
    /// whatever [`EventSink::dispatch_tree_table_controller`] itself
    /// returns.
    fn dispatch_tree_table_outcome(
        &self,
        id: WidgetId,
        outcome: TreeTableControllerEvent,
        sink: &mut dyn EventSink,
    ) -> Option<quadraui::Reaction> {
        use quadraui::Reaction;
        if matches!(outcome, TreeTableControllerEvent::Ignored) {
            return None;
        }
        Some(
            match TreeTableControllerWireEvent::from_controller_event(outcome) {
                Some(wire_event) => {
                    Reaction::Redraw.merge(sink.dispatch_tree_table_controller(id, wire_event))
                }
                None => Reaction::Redraw,
            },
        )
    }

    /// Whether the `"cell_grid"` [`PaintedHit::Widget`] painted under `id`
    /// this frame needs a fresh window of cell text —
    /// [`quadraui::CellGrid::cells_requested`] run against the SAME real
    /// widget [`dyn_widget_hit`] already resolved for dispatch, downcast
    /// back via [`quadraui::primitive::DynWidget::as_any`]. Unlike the
    /// deleted [`CellGridController`]-based version of this check, no
    /// throwaway probe is needed: that controller exposed no getter for
    /// the frozen/loaded-window fields a probe would have to reconstruct,
    /// but the real, just-decoded [`CellGrid`] already has every one of
    /// them. `None` for anything that isn't a `CellGrid` (including every
    /// other [`crate::extra_paint::WIDGET_KINDS`] member, and an `id` not
    /// painted this frame at all), and for a `CellGrid` whose loaded
    /// window already covers the resolved viewport.
    /// `pending` is the semantic event [`Self::dyn_widget_dispatch`] is
    /// about to route back to the host for the same `id`, if any — the
    /// hit-tested widget it reads via [`dyn_widget_hit`] is still this
    /// frame's painted snapshot, from *before* that event's effect (e.g.
    /// a `PageDown`'s [`CellGridEvent::Navigated`]) lands, since the real
    /// state only updates once the host re-renders. When `pending`
    /// carries a `Navigated` for this `id`, it is applied to a clone
    /// before laying out, so the loaded-window check below sees the
    /// scroll position the host is about to adopt rather than the one it
    /// is about to leave.
    fn dyn_widget_cells_requested(
        &self,
        id: WidgetId,
        pending: Option<&UiEvent>,
        backend: &mut dyn Backend,
    ) -> Option<UiEvent> {
        let (area, widget) = dyn_widget_hit(&self.hit_map.borrow(), id)?;
        let mut grid = widget.as_ref().downcast_ref::<CellGrid>()?.clone();
        if let Some(UiEvent::CellGrid(
            event_id,
            CellGridEvent::Navigated {
                cursor,
                selection_anchor,
                row_scroll,
                col_scroll,
            },
        )) = pending
        {
            if *event_id == id {
                grid.cursor = *cursor;
                grid.selection_anchor = *selection_anchor;
                grid.row_scroll = *row_scroll;
                grid.col_scroll = *col_scroll;
            }
        }
        let style = backend.style();
        let ctx = quadraui::primitive::LayoutCtx::new(
            quadraui::primitive::LayoutCtx::resolve_row_height(
                backend.chrome_line_height(),
                &style,
            ),
            backend.char_width(),
            backend.nerd_fonts_enabled(),
        );
        let widget: Rc<dyn DynWidget> = Rc::new(grid.clone());
        let layout = widget.layout(&ctx, area);
        let layout = layout.downcast_ref::<CellGridPrimitiveLayout>()?;
        let event = grid.cells_requested(&layout.layout)?;
        Some(UiEvent::CellGrid(id, event))
    }

    /// Routes `event` into the open widget overlay of `kind`, if any,
    /// through the widget's own [`DynWidget::handle`] — `None` when none is
    /// open or it does not claim `event`, so routing carries on.
    ///
    /// A modal dialog is routed every input event, laid out over the
    /// viewport; it claims whatever it handles and every press, and lets
    /// what it ignores travel on. A popup is routed the presses,
    /// double-clicks and wheel notches that land on it and claims every
    /// one of them, so none reaches whatever is painted beneath it; Escape
    /// or a press outside it dismisses it.
    ///
    /// The event the widget reports reaches `sink`; a dialog's answer
    /// ([`UiEvent::MessageDialogResolved`]) closes it, as does a
    /// completions list's activation or keyboard hand-back, and
    /// [`WidgetOverlay::apply_event`] writes descriptor-held values back.
    fn dispatch_widget_overlay(
        &mut self,
        kind: WidgetOverlayKind,
        event: &UiEvent,
        backend: &mut dyn Backend,
        sink: &mut dyn EventSink,
    ) -> Option<quadraui::Reaction> {
        use quadraui::Reaction;

        let entry = self.widget_overlay(kind)?;
        let id = entry.id();
        let bounds = self.widget_overlay_bounds(backend, entry);
        let modal = kind.is_modal();
        if !modal {
            if is_escape(event) {
                self.remove_widget_overlay(kind);
                return Some(Reaction::Redraw);
            }
            let position = match event {
                UiEvent::MouseDown { position, .. }
                | UiEvent::DoubleClick { position, .. }
                | UiEvent::Scroll { position, .. } => *position,
                _ => return None,
            };
            if !bounds.contains(position) {
                if matches!(event, UiEvent::MouseDown { .. }) {
                    self.remove_widget_overlay(kind);
                    return Some(Reaction::Redraw);
                }
                return None;
            }
        }
        let focus = if modal {
            DispatchFocus::Modal
        } else {
            DispatchFocus::Popup
        };
        let outcome = self.dyn_widget_dispatch_with_focus(
            id,
            entry.widget.as_ref(),
            bounds,
            DispatchInput::Raw(event),
            backend,
            sink,
            focus,
        )?;
        let mut reaction = outcome.reaction;
        match outcome.event {
            Some(resolved @ UiEvent::MessageDialogResolved(_)) => {
                self.close_modal_widget_overlay(kind, backend);
                reaction = reaction
                    .merge(Reaction::Redraw)
                    .merge(sink.dispatch(&resolved, None));
            }
            Some(activated @ UiEvent::CompletionItemActivated(..)) => {
                self.remove_widget_overlay(kind);
                reaction = reaction
                    .merge(Reaction::Redraw)
                    .merge(sink.dispatch(&activated, None));
            }
            Some(UiEvent::List(_, ListViewEvent::KeyPressed { .. }))
                if kind == WidgetOverlayKind::Completions =>
            {
                self.remove_widget_overlay(kind);
                reaction = reaction.merge(Reaction::Redraw);
            }
            Some(reported) => {
                let changed = self
                    .widget_overlay_mut(kind)
                    .is_some_and(|entry| entry.apply_event(&reported));
                if changed || modal {
                    reaction = reaction.merge(Reaction::Redraw);
                }
                reaction = reaction.merge(sink.dispatch(&reported, None));
            }
            None if outcome.consumed => {}
            None if modal && !matches!(event, UiEvent::MouseDown { .. }) => return None,
            None if modal => reaction = reaction.merge(Reaction::Redraw),
            None => {}
        }
        Some(reaction)
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

        for kind in [
            WidgetOverlayKind::MessageDialog,
            WidgetOverlayKind::FormDialog,
        ] {
            if let Some(reaction) = self.dispatch_widget_overlay(kind, event, backend, sink) {
                return Some(reaction);
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

        for kind in [
            WidgetOverlayKind::Completions,
            WidgetOverlayKind::RichTextPopup,
        ] {
            if let Some(reaction) = self.dispatch_widget_overlay(kind, event, backend, sink) {
                return Some(reaction);
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
        // out from under it, mirroring `FormDialog`'s own
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
    /// The [`RowsRequestedEvent`] the windowed leaf painted under `id`
    /// needs now that a scroll input left it scrolled to ABSOLUTE
    /// `scroll_offset`, or `None` when `id` isn't a windowed leaf, its
    /// loaded slice already covers the viewport, or the identical request
    /// was already made against the same loaded slice (see
    /// [`Self::rows_requested`]).
    ///
    /// The window and the viewport capacity come off the leaf's own hit
    /// ([`crate::hit::windowed_leaf_extent`]) — the frame that painted it
    /// is the only place that knew both — so this pairs an absolute offset
    /// with the window that produced it, which is
    /// [`rows_requested_for_window`]'s own precondition.
    fn rows_requested_for(&self, id: WidgetId, scroll_offset: usize) -> Option<RowsRequestedEvent> {
        let (loaded, total, viewport_rows) = windowed_leaf_extent(&self.hit_map.borrow(), id)?;
        let request = rows_requested_for_window(
            id,
            ItemsWindow {
                start: loaded.start,
                total,
            },
            loaded.len,
            scroll_offset,
            viewport_rows,
        )?;
        let asked = (loaded, request.start, request.count);
        if self.rows_requested.borrow().get(&id) == Some(&asked) {
            return None;
        }
        self.rows_requested.borrow_mut().insert(id, asked);
        Some(request)
    }

    /// Asks `sink` for the rows a windowed leaf needs after `event` — the
    /// fully-resolved event [`Self::handle`] is about to dispatch — moved
    /// its scroll offset. Every scroll input that reaches a windowed leaf
    /// resolves to one of these two events (a wheel notch, a scrollbar
    /// thumb drag and a track page all become `ScrollOffsetChanged`; a
    /// `TextDisplay` has its own typed scroll event), and both carry the
    /// new ABSOLUTE offset, which is what [`Self::rows_requested_for`]
    /// needs.
    fn dispatch_rows_requested_after_scroll(
        &self,
        event: &UiEvent,
        sink: &mut dyn EventSink,
    ) -> quadraui::Reaction {
        let scrolled = match event {
            UiEvent::ScrollOffsetChanged { widget, new_offset } => Some((*widget, *new_offset)),
            UiEvent::TextDisplay(id, TextDisplayEvent::Scrolled { new_offset }) => {
                Some((*id, *new_offset))
            }
            _ => None,
        };
        let Some((id, scroll_offset)) = scrolled else {
            return quadraui::Reaction::Continue;
        };
        match self.rows_requested_for(id, scroll_offset) {
            Some(request) => sink.dispatch_rows_requested(request),
            None => quadraui::Reaction::Continue,
        }
    }

    /// [`Self::dispatch_rows_requested_after_scroll`] for a navigation key
    /// instead of a pointer scroll: an `apply_*_nav_key` call leaves the
    /// cursor's new offset in [`Self::widget_state`] rather than on the
    /// event, so the offset is read back from there.
    fn dispatch_rows_requested_after_nav(
        &self,
        id: WidgetId,
        sink: &mut dyn EventSink,
    ) -> quadraui::Reaction {
        let Some(scroll_offset) = stored_cursor_scroll_offset(&self.widget_state, id) else {
            return quadraui::Reaction::Continue;
        };
        match self.rows_requested_for(id, scroll_offset) {
            Some(request) => sink.dispatch_rows_requested(request),
            None => quadraui::Reaction::Continue,
        }
    }

    /// Routes a non-double-click-aware `MouseDown`/`DoubleClick` landing
    /// on `HitDetail::WidgetBody` through the matched
    /// [`crate::layout::LeafDescriptor::Widget`]'s own
    /// [`DynWidget::handle`] — the generic seam's counterpart of
    /// [`Self::handle`]'s own `HitDetail::ExtraBody` arm, which instead
    /// special-cases each [`crate::extra_paint::ExtraPaint`] kind it
    /// recognizes by downcast. Resolves `widget`'s layout fresh against
    /// `area` rather than reusing anything from paint time — see
    /// [`PaintedHit::Widget`]'s own doc for why nothing is stashed to
    /// reuse. An empty [`DynWidgetClickOutcome`] (no-op) when `id` doesn't
    /// name a `PaintedHit::Widget` this frame.
    fn dispatch_dyn_widget_click(
        &self,
        id: WidgetId,
        click: PointerClick,
        backend: &mut dyn Backend,
        sink: &mut dyn EventSink,
    ) -> DynWidgetClickOutcome {
        let PointerClick {
            position,
            button,
            modifiers,
            double_click,
        } = click;
        let Some((area, widget)) = dyn_widget_hit(&self.hit_map.borrow(), id) else {
            return DynWidgetClickOutcome::default();
        };
        let local = Point::new(position.x - area.x, position.y - area.y);
        let routed = RoutedEvent::Pointer {
            kind: if double_click {
                PointerKind::DoubleClick(button)
            } else {
                PointerKind::Down(button)
            },
            sub: None,
            local,
            mods: modifiers,
        };
        self.dyn_widget_dispatch(id, &widget, area, routed, backend, sink)
    }

    /// Routes a wheel notch at `position` to the topmost
    /// `PaintedHit::Widget` whose area contains it, through that leaf's
    /// own [`DynWidget::handle`] as a [`RoutedEvent::Wheel`]. An empty
    /// [`DynWidgetClickOutcome`] (no-op) when no painted widget is under
    /// the pointer.
    fn dispatch_dyn_widget_wheel(
        &self,
        position: Point,
        delta: quadraui::ScrollDelta,
        backend: &mut dyn Backend,
        sink: &mut dyn EventSink,
    ) -> DynWidgetClickOutcome {
        let hit = self
            .hit_map
            .borrow()
            .iter()
            .rev()
            .find_map(|hit| match hit {
                PaintedHit::Widget { id, area, widget } if area.contains(position) => {
                    Some((*id, *area, Rc::clone(widget)))
                }
                _ => None,
            });
        let Some((id, area, widget)) = hit else {
            return DynWidgetClickOutcome::default();
        };
        let routed = RoutedEvent::Wheel {
            dx: delta.x,
            dy: delta.y,
            local: Point::new(position.x - area.x, position.y - area.y),
            mods: Modifiers::default(),
        };
        self.dyn_widget_dispatch(id, &widget, area, routed, backend, sink)
    }

    /// Routes a `KeyPressed`/`CharTyped`/`ClipboardPaste` to whichever
    /// [`crate::layout::LeafDescriptor::Widget`] leaf currently holds
    /// engine focus, through that leaf's own [`DynWidget::handle`] — the
    /// generic seam's counterpart of the hand-written focused-key blocks
    /// [`Self::handle`] already has for `"form"`/`"tree_table_controller"`.
    /// `CharTyped`/`ClipboardPaste` both map to [`RoutedEvent::Text`], the
    /// same shape a committed IME string arrives as — a widget whose
    /// `Interactive::handle` treats "text with no active edit" as the
    /// start of one (e.g. [`CellGrid`]) treats a paste the same way; see
    /// [`CellGridEvent`]'s own doc for why that means a paste never
    /// spreads across more than the cursor cell here, unlike the deleted
    /// [`CellGridController`]-based `PasteRequested` wire event. An empty
    /// [`DynWidgetClickOutcome`] (no-op) when nothing is focused, or the
    /// focused id doesn't name a `PaintedHit::Widget` this frame.
    fn dispatch_dyn_widget_key(
        &self,
        event: &UiEvent,
        backend: &mut dyn Backend,
        sink: &mut dyn EventSink,
    ) -> DynWidgetClickOutcome {
        let Some(focused) = backend.focus_tree().focused_id().copied() else {
            return DynWidgetClickOutcome::default();
        };
        let Some((area, widget)) = dyn_widget_hit(&self.hit_map.borrow(), focused) else {
            return DynWidgetClickOutcome::default();
        };
        let routed = match event {
            UiEvent::KeyPressed {
                key,
                modifiers,
                repeat,
                ..
            } => RoutedEvent::Key(KeyEvent {
                key: key.clone(),
                modifiers: *modifiers,
                repeat: *repeat,
            }),
            UiEvent::CharTyped(c) => RoutedEvent::Text(c.to_string()),
            UiEvent::ClipboardPaste(text) => RoutedEvent::Text(text.clone()),
            _ => return DynWidgetClickOutcome::default(),
        };
        self.dyn_widget_dispatch(focused, &widget, area, routed, backend, sink)
    }

    /// Routes a `MouseMoved`/`MouseUp` to whichever [`PaintedHit::Widget`]
    /// leaf currently holds [`Self::dyn_widget_capture`] — the generic
    /// seam's counterpart of [`Self::active_drag`]/[`Self::scrollbar_drag`]/
    /// [`Self::column_drag`]/[`Self::scroll_drag`]'s own drag-continuation
    /// routing, needed because [`EventHost::capture_pointer`]'s own
    /// contract ("route every subsequent pointer event to `id` ...
    /// regardless of where the pointer moves") has no generic
    /// implementation yet outside those four bespoke state machines. An
    /// empty [`DynWidgetClickOutcome`] (no-op) when nothing is captured,
    /// or the captured id doesn't name a `PaintedHit::Widget` this frame
    /// (it stopped painting mid-drag).
    fn dispatch_dyn_widget_captured_move(
        &self,
        position: Point,
        backend: &mut dyn Backend,
        sink: &mut dyn EventSink,
    ) -> DynWidgetClickOutcome {
        let Some(id) = *self.dyn_widget_capture.borrow() else {
            return DynWidgetClickOutcome::default();
        };
        let Some((area, widget)) = dyn_widget_hit(&self.hit_map.borrow(), id) else {
            return DynWidgetClickOutcome::default();
        };
        let routed = RoutedEvent::Pointer {
            kind: PointerKind::Move,
            sub: None,
            local: Point::new(position.x - area.x, position.y - area.y),
            mods: Modifiers::default(),
        };
        self.dyn_widget_dispatch(id, &widget, area, routed, backend, sink)
    }

    /// Routes an uncaptured pointer move's hover change to the generic
    /// leaves involved, through their own [`DynWidget::handle`]: a `Leave`
    /// to the leaf the pointer has just left, and an `Enter` to the leaf
    /// it arrives on, and a `Move` to the leaf it stays on when that leaf
    /// sets [`quadraui::interactive::Interactive::RECEIVES_HOVER_MOVES`] —
    /// the same hover tracking the native runtime does, so both report the
    /// same events. Each event a
    /// leaf reports (a `Canvas` region's `RegionHovered`, say) reaches
    /// `sink` alongside the generic `MouseEntered`/`MouseLeft` pair.
    fn route_dyn_widget_hover(
        &self,
        now: Option<WidgetId>,
        position: Point,
        backend: &mut dyn Backend,
        sink: &mut dyn EventSink,
    ) -> quadraui::Reaction {
        let prev = self.dyn_widget_hover.replace(now);
        let mut reaction = quadraui::Reaction::Continue;
        if prev == now {
            let Some(id) = now else {
                return reaction;
            };
            let receives_moves = dyn_widget_hit(&self.hit_map.borrow(), id)
                .is_some_and(|(_, widget)| widget.receives_hover_moves());
            if receives_moves {
                reaction = self.dispatch_dyn_widget_pointer(
                    id,
                    PointerKind::Move,
                    position,
                    backend,
                    sink,
                );
            }
            return reaction;
        }
        if let Some(id) = prev {
            reaction = reaction.merge(self.dispatch_dyn_widget_pointer(
                id,
                PointerKind::Leave,
                position,
                backend,
                sink,
            ));
        }
        if let Some(id) = now {
            reaction = reaction.merge(self.dispatch_dyn_widget_pointer(
                id,
                PointerKind::Enter,
                position,
                backend,
                sink,
            ));
        }
        reaction
    }

    /// Routes one pointer `kind` at `position` to the painted generic
    /// leaf `id` and dispatches whatever event it reports. A no-op when
    /// `id` did not paint this frame.
    fn dispatch_dyn_widget_pointer(
        &self,
        id: WidgetId,
        kind: PointerKind,
        position: Point,
        backend: &mut dyn Backend,
        sink: &mut dyn EventSink,
    ) -> quadraui::Reaction {
        let Some((area, widget)) = dyn_widget_hit(&self.hit_map.borrow(), id) else {
            return quadraui::Reaction::Continue;
        };
        let routed = RoutedEvent::Pointer {
            kind,
            sub: None,
            local: Point::new(position.x - area.x, position.y - area.y),
            mods: Modifiers::default(),
        };
        let outcome = self.dyn_widget_dispatch(id, &widget, area, routed, backend, sink);
        match outcome.event {
            Some(event) => outcome.reaction.merge(sink.dispatch(&event, None)),
            None => outcome.reaction,
        }
    }

    /// The focused leaf and its focused sub-part, for
    /// [`EventHost::is_focused`]: the backend's focus, with the sub-part
    /// from [`Self::dyn_widget_focus_sub`] when the backend names none and
    /// the recorded request is for the same leaf. A recorded sub-part for
    /// a leaf that has since lost focus is dropped.
    fn dyn_widget_focus(&self, backend: &dyn Backend) -> Option<quadraui::FocusAddress> {
        let (id, sub) = *backend.focus_tree().focused()?;
        let mut recorded = self.dyn_widget_focus_sub.borrow_mut();
        if recorded.is_some_and(|(rid, _)| rid != id) {
            *recorded = None;
        }
        let sub = sub.or_else(|| recorded.and_then(|(_, s)| s));
        Some((id, sub))
    }

    /// Runs `widget.handle` for one already-constructed [`RoutedEvent`]
    /// against `area`, applying whatever [`EventCx`] intents it recorded
    /// (focus/redraw/emit/pointer-capture) the same way regardless of
    /// which caller (click, focused-key, or captured-drag routing)
    /// resolved the event — and, whenever the call produced a real
    /// outcome (not a bare [`Handled::Ignored`]), also checks
    /// [`Self::dyn_widget_cells_requested`] and dispatches its own event
    /// too, mirroring the deleted [`CellGridController`]-based
    /// `dispatch_cell_grid_outcome`'s identical "whenever outcome wasn't
    /// Ignored" doc.
    fn dyn_widget_dispatch(
        &self,
        id: WidgetId,
        widget: &Rc<dyn DynWidget>,
        area: Rect,
        routed: RoutedEvent,
        backend: &mut dyn Backend,
        sink: &mut dyn EventSink,
    ) -> DynWidgetClickOutcome {
        self.dyn_widget_dispatch_with_focus(
            id,
            widget.as_ref(),
            area,
            DispatchInput::Routed(routed),
            backend,
            sink,
            DispatchFocus::Tree,
        )
        .unwrap_or_default()
    }

    /// [`Self::dyn_widget_dispatch`]'s body, for any widget: `input` is
    /// either an already-routed event or a raw one, which is routed here
    /// against the widget's own layout and sub-regions
    /// ([`quadraui::compose::routed_event`]) — `None` when a raw event is
    /// one no widget handles. `focus` says how the widget relates to the
    /// backend's focus (see [`DispatchFocus`]).
    #[allow(clippy::too_many_arguments)]
    fn dyn_widget_dispatch_with_focus(
        &self,
        id: WidgetId,
        widget: &dyn DynWidget,
        area: Rect,
        input: DispatchInput<'_>,
        backend: &mut dyn Backend,
        sink: &mut dyn EventSink,
        focus: DispatchFocus,
    ) -> Option<DynWidgetClickOutcome> {
        // The same ctx `DrawsPrimitives::draw_dyn` lays the widget out
        // with at paint time (editor line height, the backend's text
        // measurer, its style and the paint state the widget painted
        // with), so a click resolves against the geometry that was
        // actually painted.
        let style = backend.style();
        let painted = quadraui::primitive::painted_state(&id);
        let (units, layout) = {
            let backend_ref: &dyn Backend = backend;
            let measurer =
                |text: &str, role: quadraui::FontRole| backend_ref.measure_text(text, role).0;
            let ctx = quadraui::primitive::LayoutCtx::new(
                quadraui::primitive::LayoutCtx::resolve_row_height(
                    backend_ref.chrome_line_height(),
                    &style,
                ),
                backend_ref.char_width(),
                backend_ref.nerd_fonts_enabled(),
            )
            .with_editor_line_height(backend_ref.line_height());
            let ctx = ctx
                .with_measurer(&measurer)
                .with_style(&style)
                .with_paint_state(&painted);
            (ctx.units(), widget.layout(&ctx, area))
        };
        let routed = match input {
            DispatchInput::Routed(routed) => routed,
            DispatchInput::Raw(event) => {
                quadraui::compose::routed_event(event, area, &widget.sub_regions(layout.as_ref()))?
            }
        };

        let mut intents = DynWidgetIntents::default();
        let handled = {
            let mut states = self.widget_state.borrow_mut();
            let state = states.state_for(id, widget);
            let mut host = DynWidgetEventHost {
                units,
                focused: self.dyn_widget_focus(backend),
                whole_world: focus == DispatchFocus::Modal,
                intents: &mut intents,
            };
            let mut cx = EventCx::new(id, &mut host);
            widget.handle(state, &routed, layout.as_ref(), &mut cx)
        };

        let mut outcome = DynWidgetClickOutcome::default();
        if let Some((focus_id, sub)) = intents
            .focus_request
            .filter(|_| focus == DispatchFocus::Tree)
        {
            *self.dyn_widget_focus_sub.borrow_mut() = Some((focus_id, sub));
            if backend.set_focus(Some(focus_id)) {
                let focus_reaction = sink.dispatch(&UiEvent::FocusChanged(Some(focus_id)), None);
                outcome.reaction = outcome.reaction.merge(focus_reaction);
            }
        }
        if intents.capture_requested {
            *self.dyn_widget_capture.borrow_mut() = Some(id);
        }
        if intents.release_requested {
            *self.dyn_widget_capture.borrow_mut() = None;
        }
        if intents.redraw {
            outcome.reaction = outcome.reaction.merge(quadraui::Reaction::Redraw);
        }
        for emitted in intents.emitted {
            let reaction = sink.dispatch(&emitted, Some(&HitDetail::WidgetBody));
            outcome.reaction = outcome.reaction.merge(reaction);
        }
        match handled {
            Handled::Event(replacement) => outcome.event = Some(replacement),
            Handled::Consumed => {
                outcome.consumed = true;
                outcome.reaction = outcome.reaction.merge(quadraui::Reaction::Redraw);
            }
            Handled::Ignored => {}
        }

        if outcome.event.is_some() || outcome.consumed {
            if let Some(cells_requested) =
                self.dyn_widget_cells_requested(id, outcome.event.as_ref(), backend)
            {
                let reaction = sink.dispatch(&cells_requested, Some(&HitDetail::WidgetBody));
                outcome.reaction = outcome.reaction.merge(reaction);
            }
        }
        Some(outcome)
    }

    /// Routes one input event and returns how the host should react.
    ///
    /// Every event first feeds this thread's ambient pointer state
    /// ([`quadraui::interaction::track_pointer`]), which the widgets read
    /// at paint time for hover and press; a change there asks for a
    /// redraw whatever the routing below returns. Feeding an event the
    /// host's runner already tracked is a no-op, so a host may route
    /// through both.
    pub fn handle(
        &mut self,
        event: UiEvent,
        backend: &mut dyn Backend,
        sink: &mut dyn EventSink,
    ) -> quadraui::Reaction {
        let pointer = if quadraui::interaction::track_pointer(&event) {
            quadraui::Reaction::Redraw
        } else {
            quadraui::Reaction::Continue
        };
        pointer.merge(self.route(event, backend, sink))
    }

    fn route(
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

        // The reaction a focused generic widget produced for a key it let
        // travel on (`Handled::Ignored`) while still asking for a repaint —
        // a script-defined widget whose script consumed the key does this
        // so the app sees the key press too. Merged into the final
        // `Reaction` below, the same way `tooltip_dismissed` is, so the
        // widget's repainted state shows even when `sink.dispatch` itself
        // leaves `event` unhandled.
        let mut widget_key_reaction = Reaction::Continue;

        // Set when an armed `Self::gesture_drag` ends (`MouseUp`) over a
        // valid drop target, below — merged into the final `Reaction` the
        // same way `widget_key_reaction` is. `drag_started_reaction`/
        // `drag_over_reaction` are its arm/continue counterparts, set
        // further down once `hit`/the `MouseMoved` branch are reached.
        let mut gesture_reaction = Reaction::Continue;
        let mut drag_started_reaction = Reaction::Continue;
        let mut drag_over_reaction = Reaction::Continue;

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
            // Ends an armed `GestureDrag` the same way, dispatching
            // `Dropped` first when it ended over a valid drop target (its
            // own `target` is `Some`) — the gesture is cleared either way,
            // so a `MouseUp` over nothing droppable still ends it, just
            // silently (see `GestureDrag::target`'s own doc).
            if let Some(gesture) = self.gesture_drag.borrow_mut().take() {
                if let Some((target, target_detail)) = gesture.target {
                    gesture_reaction = sink.dispatch_drag_gesture(DragGestureWireEvent::Dropped {
                        source: gesture.source,
                        source_detail: gesture.source_detail,
                        target,
                        target_detail,
                    });
                }
            }
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
            // Same reset for a `"tree_table_controller"` extra's own
            // scrollbar-thumb drag — `TreeTableController::handle`'s own
            // `MouseUp` arm ends it unconditionally too.
            for id in self.tree_table_order.borrow().iter() {
                let ctx = self.tree_table_rects.borrow().get(id).cloned();
                if let Some((rect, columns)) = ctx {
                    if let Some(controller) = self.tree_table_controllers.borrow_mut().get_mut(id) {
                        let _ = controller.handle(&event, backend, rect, columns);
                    }
                }
            }
            // Ends a [`Self::dyn_widget_capture`] drag the same way — a
            // captured `DynWidget` (e.g. `CellGrid`'s own
            // drag-to-extend-selection) gets this `MouseUp` routed to it
            // first, so its own `RoutedEvent::Pointer { kind:
            // PointerKind::Up, .. }` arm can clear whatever drag state it
            // holds (`CellGridState::drag_anchor`), then the capture is
            // cleared unconditionally regardless of what `handle`
            // reported — mirrors every reset above: a release outside the
            // widget's own rect still ends it.
            let captured = self.dyn_widget_capture.borrow_mut().take();
            if let Some(id) = captured {
                let hit = dyn_widget_hit(&self.hit_map.borrow(), id);
                if let Some((area, widget)) = hit {
                    if let UiEvent::MouseUp {
                        position, button, ..
                    } = &event
                    {
                        let routed = RoutedEvent::Pointer {
                            kind: PointerKind::Up(*button),
                            sub: None,
                            local: Point::new(position.x - area.x, position.y - area.y),
                            mods: Modifiers::default(),
                        };
                        let outcome =
                            self.dyn_widget_dispatch(id, &widget, area, routed, backend, sink);
                        if let Some(further) = outcome.event {
                            let _ = sink.dispatch(&further, Some(&HitDetail::WidgetBody));
                        }
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

        // A widget popup (an open `ComboBox` list) paints above every
        // leaf and overlay, but this host does not route popup events to
        // the popup's owner, so a press, release, double-click or wheel
        // landing on one stops here instead of reaching what it covers.
        let on_popup = |p: Point| {
            quadraui::popup::painted_popups()
                .iter()
                .any(|popup| popup.rect.contains(p))
        };
        if matches!(
            event,
            UiEvent::MouseDown { .. }
                | UiEvent::MouseUp { .. }
                | UiEvent::DoubleClick { .. }
                | UiEvent::Scroll { .. }
        ) && event_position(&event).is_some_and(on_popup)
        {
            return continue_or_tooltip_redraw(tooltip_dismissed);
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
        // `AppMenu::handle_ui_event`/`ContextMenuController::handle`'s own doc
        // ("call this before other UI routing"). Each reports `Ignored`
        // immediately when nothing of theirs is open/relevant, so this is
        // safe to run unconditionally even for an app with neither a menu
        // bar nor an open context menu.
        let mut menu_reaction = Reaction::Continue;
        if let Some(bar_rect) = *self.menu_bar_rect.borrow() {
            let was_open = self.menu_system.borrow().menu.is_open();
            let menu_event = {
                let mut host = self.menu_system.borrow_mut();
                let MenuBarHost { menu, state } = &mut *host;
                menu.handle_ui_event(state, &event, backend, bar_rect)
            };
            match menu_event {
                Handled::Event(AppMenuEvent::Activated(id)) => {
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
                Handled::Event(_) | Handled::Consumed => {
                    let hover_only_on_closed_bar = !was_open
                        && !self.menu_system.borrow().menu.is_open()
                        && matches!(event, UiEvent::MouseMoved { .. });
                    if hover_only_on_closed_bar {
                        menu_reaction = Reaction::Redraw;
                    } else {
                        return Reaction::Redraw;
                    }
                }
                Handled::Ignored => {}
            }
        }
        match self.dispatch_context_menu(&event, backend) {
            ContextMenuOutcome::Event(ev) => {
                // Same reasoning as the `AppMenuEvent::Activated` arm above:
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
            if let Some(focused) = backend.focus_tree().focused_id().copied() {
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

        // A `KeyPressed` landing on a focused `"tree_table_controller"`
        // extra routes through its own cached [`TreeTableController`] —
        // `TreeTableController::handle` does its own internal
        // `Ignored`-when-nothing-to-do gate (it has no generic `KeyPressed`
        // arm at all beyond [`crate::host`]'s own navigation keys), so
        // trying this unconditionally is safe even when no tree-table
        // controller is focused: `tree_table_rects` then has no entry for
        // the focused id. Unlike `"form"`, there is no Tab/Shift-Tab
        // special case — [`TreeTableController::handle`] never claims
        // Tab/BackTab, so the generic cross-widget cycle already handles
        // leaving it.
        if matches!(event, UiEvent::KeyPressed { .. }) {
            if let Some(focused) = backend.focus_tree().focused_id().copied() {
                let ctx = self.tree_table_rects.borrow().get(&focused).cloned();
                if let Some((rect, columns)) = ctx {
                    let outcome = self
                        .tree_table_controllers
                        .borrow_mut()
                        .get_mut(&focused)
                        .map(|controller| {
                            controller.set_has_focus(true);
                            controller.handle(&event, backend, rect, columns)
                        });
                    if let Some(outcome) = outcome {
                        if let Some(reaction) =
                            self.dispatch_tree_table_outcome(focused, outcome, sink)
                        {
                            return reaction.merge(continue_or_tooltip_redraw(tooltip_dismissed));
                        }
                    }
                }
            }
        }

        // A `KeyPressed`/`CharTyped`/`ClipboardPaste` landing on a
        // focused [`crate::extra_paint::WIDGET_KINDS`] leaf (e.g. a
        // `"cell_grid"`) routes through its own [`DynWidget::handle`] —
        // the generic seam's counterpart of the `"tree_table_controller"`
        // block above. `DynWidget::handle` reports [`Handled::Ignored`]
        // for anything it doesn't recognize, so trying this
        // unconditionally is safe even when nothing focused names a
        // `PaintedHit::Widget` this frame. `CharTyped`/`ClipboardPaste`
        // reach this block (unlike the plain `KeyPressed`-only tree-table
        // block above) because a widget like [`CellGrid`] treats both as
        // the [`RoutedEvent::Text`] a committed IME string would produce
        // — see [`Self::dispatch_dyn_widget_key`]'s own doc for why that
        // means a paste never spreads past the cursor cell.
        if matches!(
            event,
            UiEvent::KeyPressed { .. } | UiEvent::CharTyped(_) | UiEvent::ClipboardPaste(_)
        ) {
            let outcome = self.dispatch_dyn_widget_key(&event, backend, sink);
            if matches!(outcome.reaction, Reaction::Exit) {
                return outcome.reaction;
            }
            if let Some(replacement) = outcome.event {
                let reaction = sink.dispatch(&replacement, Some(&HitDetail::WidgetBody));
                return outcome
                    .reaction
                    .merge(reaction)
                    .merge(continue_or_tooltip_redraw(tooltip_dismissed));
            } else if outcome.consumed {
                return outcome
                    .reaction
                    .merge(continue_or_tooltip_redraw(tooltip_dismissed));
            }
            widget_key_reaction = outcome.reaction;
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

        // A `MouseDown` or a `DoubleClick` (the second click of a pair
        // `DoubleClickDetector` folded, e.g. the mouse half of
        // `TreeTableControllerEvent::RowActivated`) lands on at most one
        // `"tree_table_controller"` extra's own `rect` —
        // [`TreeTableController::handle`]'s own `click`/`click_scrollbar`
        // return `Ignored` for a position outside it — so trying every
        // cached controller, in `tree_table_order`'s deterministic paint
        // order, and stopping at the first non-`Ignored` result mirrors
        // the `"form"` claim loop just above.
        if matches!(
            event,
            UiEvent::MouseDown { .. } | UiEvent::DoubleClick { .. }
        ) {
            let claim = self
                .tree_table_order
                .borrow()
                .iter()
                .filter_map(|id| {
                    self.tree_table_rects
                        .borrow()
                        .get(id)
                        .cloned()
                        .map(|ctx| (*id, ctx))
                })
                .collect::<Vec<_>>()
                .into_iter()
                .find_map(|(id, (rect, columns))| {
                    let outcome = self
                        .tree_table_controllers
                        .borrow_mut()
                        .get_mut(&id)
                        .map(|controller| controller.handle(&event, backend, rect, columns))?;
                    (!matches!(outcome, TreeTableControllerEvent::Ignored)).then_some((id, outcome))
                });
            if let Some((id, outcome)) = claim {
                // Mirrors the `"form"` claim loop above: a click a
                // `TreeTableController` just claimed also moves
                // engine-level focus to the extra's own id, so the
                // `KeyPressed` routing above can find it on the very next
                // keystroke.
                let mut focus_reaction = Reaction::Continue;
                if backend.set_focus(Some(id)) {
                    focus_reaction = sink.dispatch(&UiEvent::FocusChanged(Some(id)), None);
                    if matches!(focus_reaction, Reaction::Exit) {
                        return focus_reaction;
                    }
                }
                let reaction = self
                    .dispatch_tree_table_outcome(id, outcome, sink)
                    .unwrap_or(Reaction::Redraw);
                return focus_reaction.merge(reaction);
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

        // A move with the left button held continues whichever
        // `"tree_table_controller"` extra's own scrollbar-thumb drag (if
        // any) the `MouseDown` claim above armed — mirrors the `"form"`
        // block just above; [`TreeTableController::handle`]'s own
        // `MouseMoved` arm no-ops (`Ignored`) for every controller with no
        // active drag.
        if matches!(
            event,
            UiEvent::MouseMoved {
                buttons: quadraui::ButtonMask { left: true, .. },
                ..
            }
        ) {
            let mut consumed = false;
            for id in self.tree_table_order.borrow().iter() {
                let ctx = self.tree_table_rects.borrow().get(id).cloned();
                if let Some((rect, columns)) = ctx {
                    if let Some(controller) = self.tree_table_controllers.borrow_mut().get_mut(id) {
                        if matches!(
                            controller.handle(&event, backend, rect, columns),
                            TreeTableControllerEvent::Consumed
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

        // A wheel `Scroll` landing inside a `"tree_table_controller"`
        // extra's own rect scrolls it directly through the cached
        // `TreeTableController` — mirrors the `"form"` block just above.
        if let UiEvent::Scroll { position, .. } = &event {
            let claim = self
                .tree_table_order
                .borrow()
                .iter()
                .filter_map(|id| {
                    self.tree_table_rects
                        .borrow()
                        .get(id)
                        .cloned()
                        .map(|ctx| (*id, ctx))
                })
                .collect::<Vec<_>>()
                .into_iter()
                .find(|(_, (rect, _))| rect.contains(*position));
            if let Some((id, (rect, columns))) = claim {
                if let Some(controller) = self.tree_table_controllers.borrow_mut().get_mut(&id) {
                    if matches!(
                        controller.handle(&event, backend, rect, columns),
                        TreeTableControllerEvent::Consumed
                    ) {
                        return Reaction::Redraw;
                    }
                }
            }
        }

        // A wheel `Scroll` over a [`crate::extra_paint::WIDGET_KINDS`]
        // leaf goes to the topmost painted widget under the pointer as a
        // [`RoutedEvent::Wheel`], the same hit-under-cursor rule the Rust
        // runtime's widget wheel routing follows. A widget that ignores
        // the wheel falls through to the scroll handling below.
        if let UiEvent::Scroll {
            position, delta, ..
        } = &event
        {
            let outcome = self.dispatch_dyn_widget_wheel(*position, *delta, backend, sink);
            if matches!(outcome.reaction, Reaction::Exit) {
                return outcome.reaction;
            }
            if let Some(replacement) = outcome.event {
                let reaction = sink.dispatch(&replacement, Some(&HitDetail::WidgetBody));
                return outcome
                    .reaction
                    .merge(reaction)
                    .merge(continue_or_tooltip_redraw(tooltip_dismissed));
            } else if outcome.consumed {
                return outcome
                    .reaction
                    .merge(continue_or_tooltip_redraw(tooltip_dismissed));
            }
        }

        // A `MouseDown`/click-to-focus on a `"cell_grid"` leaf no longer
        // needs a bespoke claim loop here: it now decodes as a
        // `crate::layout::LeafDescriptor::Widget`, so it hits
        // `HitDetail::WidgetBody` through the generic hit-test pipeline
        // below, the same as every other `WIDGET_KINDS` member —
        // [`Self::dispatch_dyn_widget_click`] (and, through it,
        // [`Self::dyn_widget_dispatch`]'s own focus-request handling)
        // covers click-to-focus generically. No `UiEvent::Scroll` arm
        // either: [`CellGrid::handle`][quadraui::interactive::Interactive::handle]
        // has no `RoutedEvent::Wheel` arm of its own yet (always
        // `Handled::Ignored`), same gap the deleted
        // [`CellGridController`]-based wheel block already documented as
        // a no-op.
        //
        // A move with the left button held continues whichever
        // [`crate::extra_paint::WIDGET_KINDS`] leaf's own in-progress
        // drag (if any) a prior `MouseDown` armed via
        // [`EventHost::capture_pointer`] — the generic counterpart of the
        // `"form"`/`"tree_table_controller"` scrollbar-drag-continuation
        // blocks above, routed through [`Self::dispatch_dyn_widget_captured_move`]
        // rather than a bare `Consumed` check since a widget like
        // [`CellGrid`] can report a real `Navigated` (selection-extending)
        // event as the dragged range grows.
        if let UiEvent::MouseMoved { position, .. } = &event {
            let outcome = self.dispatch_dyn_widget_captured_move(*position, backend, sink);
            if matches!(outcome.reaction, Reaction::Exit) {
                return outcome.reaction;
            }
            if let Some(replacement) = outcome.event {
                let reaction = sink.dispatch(&replacement, Some(&HitDetail::WidgetBody));
                return outcome.reaction.merge(reaction);
            } else if outcome.consumed {
                return outcome.reaction;
            }
        }

        // Whole-widget hover bookkeeping — see `Self::hover`'s own doc.
        // Synthesizes `UiEvent::MouseEntered`/`UiEvent::MouseLeft` through
        // `sink` so a binding's own handler gets a real hit-tested hover
        // signal instead of re-deriving hit-testing itself from the raw
        // `MouseMoved` position, at `hit_test_painted`'s full granularity
        // (toolbar button, tree row, list item, details-pane body, ...). Each
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
            || self.scroll_drag.borrow().is_some()
            || self.gesture_drag.borrow().is_some();
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
                let widget_now = self.hover.borrow().as_ref().and_then(|(id, detail)| {
                    matches!(detail, HitDetail::WidgetBody).then_some(*id)
                });
                hover_reaction = hover_reaction
                    .merge(self.route_dyn_widget_hover(widget_now, *position, backend, sink));
                if matches!(hover_reaction, Reaction::Exit) {
                    return hover_reaction;
                }
            }
        }

        // A move while a `Self::gesture_drag` is armed updates its own
        // `target` by re-hit-testing `position` against this frame's
        // `Self::drag_roles` (via `gesture_target_at`), dispatching
        // `DragGestureWireEvent::DragOver` through
        // `sink.dispatch_drag_gesture` whenever that target changes —
        // mirrors the hover-change bookkeeping just above, but scoped to
        // drop-target leaves only and reported through the gesture's own
        // dedicated `EventSink` method rather than `MouseEntered`/
        // `MouseLeft`. Unlike the divider/scrollbar/column drags below,
        // this never rewrites `event` itself — see `GestureDrag`'s own
        // doc for why.
        if let UiEvent::MouseMoved { position, .. } = &event {
            let dispatch_over = {
                let mut gesture = self.gesture_drag.borrow_mut();
                gesture.as_mut().and_then(|active| {
                    let new_target = gesture_target_at(
                        &self.hit_map.borrow(),
                        &self.drag_roles.borrow(),
                        *position,
                    );
                    if new_target == active.target {
                        return None;
                    }
                    active.target = new_target.clone();
                    new_target.map(|(target, target_detail)| {
                        (
                            active.source,
                            active.source_detail.clone(),
                            target,
                            target_detail,
                        )
                    })
                })
            };
            if let Some((source, source_detail, target, target_detail)) = dispatch_over {
                drag_over_reaction = sink.dispatch_drag_gesture(DragGestureWireEvent::DragOver {
                    source,
                    source_detail,
                    target,
                    target_detail,
                });
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
            if let Some(focused) = backend.focus_tree().focused_id().copied() {
                let nav_applied = if let Some((total_rows, viewport_rows)) =
                    tree_total_rows_and_viewport(&self.hit_map.borrow(), focused)
                {
                    // A WINDOWED tree, tried before the path-based
                    // `apply_tree_nav_key` below: only the loaded slice's
                    // own rows reach that one, so `End` on a 100,000-row
                    // tree with 50 rows loaded could never move past row
                    // 49. `apply_tree_nav_key_windowed` tracks the cursor
                    // by absolute index over the collection's real extent
                    // instead (`total_rows`), which the next render's
                    // `crate::host::resolve_tree_cursor_windowed` call
                    // turns back into a scroll offset and a selected path.
                    apply_tree_nav_key_windowed(
                        &self.widget_state,
                        focused,
                        total_rows,
                        viewport_rows,
                        key,
                    )
                } else if let Some((row_paths, viewport_rows)) =
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
                    // A windowed leaf's cursor move can scroll past the
                    // slice its app has loaded — ask for the rows that
                    // move brought into view, right after the key itself
                    // reaches the app.
                    let dispatched = Reaction::Redraw.merge(sink.dispatch(&event, None));
                    return dispatched.merge(self.dispatch_rows_requested_after_nav(focused, sink));
                }
            }
        }

        // IME/compose and clipboard-paste text has no `KeyPressed` of its
        // own to route through `focus_scoped_key_event` above — resolve it
        // the same way, directly against whichever `TextInput` extra
        // currently holds focus.
        if let UiEvent::CharTyped(ch) = &event {
            if let Some(focused) = backend.focus_tree().focused_id().copied() {
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
            if let Some(focused) = backend.focus_tree().focused_id().copied() {
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

        // A `MouseDown` landing on a leaf `Self::drag_roles` marks
        // `DragRole::draggable` arms `Self::gesture_drag` and dispatches
        // `DragGestureWireEvent::DragStarted` — tracked as a side effect of
        // the press, independently of whatever semantic click event the
        // same `MouseDown` also produces further below (e.g. a draggable
        // `List` row still gets its own `ListViewEvent::SelectionChanged`;
        // "pick it up" and "select it" are not mutually exclusive effects
        // of one press). Mirrors `Self::active_drag`'s own arm timing,
        // right after `hit`/`set_event_widget` resolve the id this
        // `MouseDown` landed on.
        if let (UiEvent::MouseDown { .. }, Some((id, detail))) = (&event, &hit) {
            if self
                .drag_roles
                .borrow()
                .get(id)
                .is_some_and(|role| role.draggable)
            {
                *self.gesture_drag.borrow_mut() = Some(GestureDrag {
                    source: *id,
                    source_detail: detail.clone(),
                    target: None,
                });
                drag_started_reaction =
                    sink.dispatch_drag_gesture(DragGestureWireEvent::DragStarted {
                        source: *id,
                        source_detail: detail.clone(),
                    });
            }
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
        // Set by the `HitDetail::WidgetBody` arm below.
        let mut widget_click_reaction = Reaction::Continue;
        if let Some((id, detail)) = &hit {
            let modifiers_and_double_click = match &event {
                UiEvent::MouseDown {
                    modifiers, button, ..
                } => Some((*modifiers, false, *button)),
                UiEvent::DoubleClick { .. } => {
                    Some((Modifiers::default(), true, MouseButton::default()))
                }
                _ => None,
            };
            if let Some((modifiers, double_click, button)) = modifiers_and_double_click {
                // A non-double click on any extra's body places a text
                // caret first (a side effect on `text_editors` only, with
                // no event to swap in; see `text_input_click_event`), then
                // resolves through `semantic_event_for_hit`'s `ExtraBody`
                // arm. Keeping every `ExtraBody` outcome in this one arm
                // means two of them can never fire for the same click.
                if !double_click && matches!(detail, HitDetail::ExtraBody) {
                    if let Some(position) = event_position(&event) {
                        text_input_click_event(
                            &self.hit_map.borrow(),
                            *id,
                            position,
                            quadraui::TextInputMeasure::from_metrics(&backend.measure()),
                            &self.text_editors,
                        );
                        if let Some(semantic) = semantic_event_for_hit(
                            *id,
                            detail,
                            modifiers,
                            double_click,
                            &self.hit_map.borrow(),
                        ) {
                            event = semantic;
                            attach_hit = false;
                        }
                    }
                } else if matches!(detail, HitDetail::WidgetBody) {
                    if let Some(position) = event_position(&event) {
                        let outcome = self.dispatch_dyn_widget_click(
                            *id,
                            PointerClick {
                                position,
                                button,
                                modifiers,
                                double_click,
                            },
                            backend,
                            sink,
                        );
                        if matches!(outcome.reaction, Reaction::Exit) {
                            return outcome.reaction;
                        }
                        widget_click_reaction = widget_click_reaction.merge(outcome.reaction);
                        if let Some(replacement) = outcome.event {
                            event = replacement;
                            attach_hit = false;
                        } else if outcome.consumed {
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
                    // `TreeNavCore::press_scrollbar`'s
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
                } else if let Some(semantic) = semantic_event_for_hit(
                    *id,
                    detail,
                    modifiers,
                    double_click,
                    &self.hit_map.borrow(),
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
        // After the app has seen the scroll itself: a wheel notch, a
        // scrollbar drag or a track page on a windowed leaf can leave its
        // viewport outside the loaded slice, and the app can only paint
        // those rows once it's been asked for them.
        let rows_requested_reaction = self.dispatch_rows_requested_after_scroll(&event, sink);
        let tooltip_reaction = if tooltip_dismissed {
            Reaction::Redraw
        } else {
            Reaction::Continue
        };
        tooltip_reaction
            .merge(rows_requested_reaction)
            .merge(scroll_into_view_reaction)
            .merge(menu_reaction)
            .merge(hover_reaction)
            .merge(focus_reaction)
            .merge(widget_click_reaction)
            .merge(event_reaction)
            .merge(persisted_state)
            .merge(widget_key_reaction)
            .merge(gesture_reaction)
            .merge(drag_started_reaction)
            .merge(drag_over_reaction)
    }
}

/// Builds a [`MessageDialogOptions`] from a binding's own decoded
/// `title`/`body`/`buttons`/`severity` arguments, validating `buttons` the
/// way every binding's own `show_message_dialog`-style surface needs to:
/// non-empty (a dialog with no buttons can never be resolved by Enter or
/// Escape) and no button setting `tint` (accepted for shape compatibility
/// with the in-canvas `Dialog` primitive's own `buttons` field, but not yet
/// honored by [`MessageDialog`]). `spec`, unlike `tint`, carries
/// straight through to the built [`MessageDialogButton`] — honored by
/// [`MessageDialog`] on TUI, silently ignored by the native
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

// ─── Render context for script widgets ──────────────────────────
//
// `measure_text`/`line_height`/`char_width` reach the live `&dyn Backend`
// directly at whichever call site already holds one (a binding's own
// `RenderCtx` wrapper, constructed fresh each `AppLogic::render` call —
// see `quadraui-py`'s/`quadraui-node`'s own type of that name). The two
// functions below are the shared half every binding's "theme/style token
// by name" accessor delegates to instead of hand-rolling its own
// field-by-field match over `Theme`'s/`Style`'s dozens of fields: both
// types already derive `Serialize`, so a generic `serde_json::to_value`
// lookup is exhaustive by construction — a future field on either becomes
// queryable here for free, with no match arm to add or keep in sync.

/// Looks up colour role `name` on `theme` — one of its own field names
/// (`"background"`, `"accent_fg"`, `"selected_bg"`, …) — returning its
/// `{"r":.., "g":.., "b":.., "a":..}` JSON shape, or `None` if `name` isn't
/// a field [`quadraui::Theme`] declares.
pub fn theme_token(theme: &quadraui::Theme, name: &str) -> Option<serde_json::Value> {
    let value = serde_json::to_value(theme).ok()?;
    value.as_object()?.get(name).cloned()
}

/// Looks up geometry token `name` on `style` — one of its own field names
/// (`"focus_ring_width"`, `"padding"`, `"corner_radius"`, …) — returning
/// its plain JSON number (or boolean, for a non-numeric future token), or
/// `None` if `name` isn't a field [`quadraui::Style`] declares.
pub fn style_token(style: &quadraui::Style, name: &str) -> Option<serde_json::Value> {
    let value = serde_json::to_value(style).ok()?;
    value.as_object()?.get(name).cloned()
}

#[cfg(test)]
mod tests {
    // Tests exercise the deprecated row-family `Backend::draw_*`/`*_layout`
    // methods directly, alongside their `DrawsPrimitives::draw_primitive`
    // replacements — this deprecation targets callers, not
    // this file's own coverage of the methods it deprecates.
    #![allow(deprecated)]
    use std::cell::RefCell;
    use std::collections::HashMap;

    use std::rc::Rc;

    use quadraui::primitive::DynWidget;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::{
        AppLogic, Backend, Button, Cell, CellGrid, CellGridEvent, Column, ColumnAlign, ColumnWidth,
        DataTable, Key, ListView, Modifiers, MouseButton, NamedKey, Point, Reaction, Rect,
        ScrollAxis, ScrollDelta, Split, StyledSpan, StyledText, TabBar, TabBarEvent, TabBarHit,
        TextDisplay, TextDisplayLine, TextInput, ToolbarEvent, TreeEvent, TreeView, UiEvent,
        WidgetId,
    };

    use super::{
        style_token, theme_token, viewport_rect, EventSink, InteractionEngine,
        TreeTableControllerWireEvent,
    };
    use crate::drag::find_split_tree_layout;
    use crate::extra_paint::{TreeTableControllerDescriptor, TreeTableControllerNode};
    use crate::hit::{HitDetail, PaintedHit};
    use crate::host::{
        render_overlays, resolve_data_table_cursor, resolve_layout_node,
        resolve_layout_tabs_active, resolve_list_cursor, resolve_split_ratio,
        resolve_tab_bar_active, resolve_tree_cursor, resolve_tree_expansion,
    };
    use crate::layout::{
        LayoutContainer, LayoutLeaf, LayoutNode, LayoutScroll, LayoutSplit, LayoutSplitTree,
        LayoutTabEntry, LayoutTabs, LeafDescriptor,
    };
    use crate::render::{
        draw_bands, list_viewport_rows, paint_layout, tree_viewport_rows, RenderRefs,
    };
    use crate::semantic::DragGestureWireEvent;
    use quadraui::Spacing;

    /// Records every event [`InteractionEngine::handle`] dispatches, in
    /// order, so a test can assert on the orchestration's decisions (e.g.
    /// click-to-focus sending a `FocusChanged` ahead of the click itself)
    /// without any host-language callback in scope.
    #[derive(Default)]
    struct RecordingSink {
        dispatched: Vec<(UiEvent, Option<HitDetail>)>,
        /// Every [`EventSink::dispatch_tree_table_controller`] call, in
        /// order — mirrors [`Self::dispatched`] for the one `EventSink`
        /// method whose payload isn't a [`UiEvent`] (see that trait
        /// method's own doc for why).
        tree_table_dispatched: Vec<(WidgetId, TreeTableControllerWireEvent)>,
        /// Every [`EventSink::dispatch_drag_gesture`] call, in order —
        /// mirrors [`Self::tree_table_dispatched`] for the drag-gesture
        /// counterpart.
        drag_gesture_dispatched: Vec<DragGestureWireEvent>,
    }

    impl EventSink for RecordingSink {
        fn dispatch(&mut self, event: &UiEvent, hit_detail: Option<&HitDetail>) -> Reaction {
            self.dispatched.push((event.clone(), hit_detail.cloned()));
            Reaction::Continue
        }

        fn dispatch_tree_table_controller(
            &mut self,
            id: WidgetId,
            event: TreeTableControllerWireEvent,
        ) -> Reaction {
            self.tree_table_dispatched.push((id, event));
            Reaction::Continue
        }

        fn dispatch_drag_gesture(&mut self, event: DragGestureWireEvent) -> Reaction {
            self.drag_gesture_dispatched.push(event);
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
            let mut widgets: HashMap<String, Rc<dyn DynWidget>> = HashMap::new();
            widgets.insert("button".to_string(), Rc::new(self.button.clone()));
            draw_bands(
                backend,
                RenderRefs {
                    text_display: None,
                    list: None,
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                    widgets: &widgets,
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
            );
            self.engine.render_overlays(backend);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
                    text_display: None,
                    list: None,
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                    widgets: crate::render::no_widgets(),
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
            );
            self.engine.sync_forms(backend, &hits);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
            let mut widgets: HashMap<String, Rc<dyn DynWidget>> = HashMap::new();
            widgets.insert("button".to_string(), Rc::new(self.button.clone()));
            draw_bands(
                backend,
                RenderRefs {
                    text_display: None,
                    list: None,
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                    widgets: &widgets,
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
            );
            self.engine.sync_forms(backend, &hits);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
                        descriptor: LeafDescriptor::Widget {
                            kind: "button",
                            widget: Rc::new(Button::new(WidgetId::new("before"), "Before")),
                        },
                        scroll_px: None,
                    }),
                    LayoutNode::Leaf(LayoutLeaf {
                        id: "settings".to_string(),
                        descriptor: LeafDescriptor::Extra(Rc::new(self.form.clone())),
                        scroll_px: None,
                    }),
                    LayoutNode::Leaf(LayoutLeaf {
                        id: "after".to_string(),
                        descriptor: LeafDescriptor::Widget {
                            kind: "button",
                            widget: Rc::new(Button::new(WidgetId::new("after"), "After")),
                        },
                        scroll_px: None,
                    }),
                ],
                Spacing::default(),
            ));
            let mut hits = Vec::new();
            paint_layout(backend, area, &content, &mut hits);
            self.engine.sync_forms(backend, &hits);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// A single `"tree_table_controller"` extra, filling the whole
    /// viewport via a bare [`LayoutNode::Leaf`] (same embedding
    /// [`FormBetweenTwoButtonsLogic`] uses, minus the sibling buttons) —
    /// this per-widget-id controller cache's own end-to-end acceptance
    /// coverage: click/key routing through [`InteractionEngine::handle`],
    /// expand → `ChildrenRequested` →
    /// [`InteractionEngine::set_tree_table_children`] → header-click sort
    /// → selection survives the sort by node id, all through the real
    /// `event -> hit -> semantic` path, with no host-language FFI type in
    /// scope.
    struct TreeTableControllerLogic {
        descriptor: TreeTableControllerDescriptor,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for TreeTableControllerLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let area = viewport_rect(backend);
            let content = LayoutNode::Leaf(LayoutLeaf {
                id: self.descriptor.id.as_str().to_string(),
                descriptor: LeafDescriptor::Extra(Rc::new(self.descriptor.clone())),
                scroll_px: None,
            });
            let mut hits = Vec::new();
            paint_layout(backend, area, &content, &mut hits);
            render_overlays(&self.engine, backend, hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// A single lazy-branch root `"dir"` — matches
    /// [`quadraui::TreeTableNodeData::lazy_branch`]'s own "has_children,
    /// no loaded children yet" shape, seeding a fresh
    /// [`TreeTableControllerLogic`].
    fn lazy_root_descriptor() -> TreeTableControllerDescriptor {
        TreeTableControllerDescriptor {
            id: WidgetId::new("tt"),
            columns: vec![Column {
                title: "Name".into(),
                width: ColumnWidth::Flex(1.0),
                align: ColumnAlign::Left,
            }],
            nodes: vec![TreeTableControllerNode {
                id: "dir".to_string(),
                cells: vec![StyledText::plain("dir")],
                icon: None,
                has_children: true,
                children: None,
            }],
            selected_id: Some("dir".to_string()),
            has_focus: true,
            show_scrollbar: true,
            bordered: false,
            folders_first: false,
        }
    }

    /// End-to-end acceptance coverage for the `TreeTableController`
    /// per-widget-id seam: expand a lazy branch → exactly one
    /// `ChildrenRequested` → [`InteractionEngine::set_tree_table_children`]
    /// fills the level → a header click sorts children under their
    /// parent, not into one flat list → selection survives the sort by
    /// node id, not by its (now-changed) position.
    #[test]
    fn tree_table_controller_expand_fetch_sort_and_selection_round_trip() {
        let logic = TreeTableControllerLogic {
            descriptor: lazy_root_descriptor(),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        // The row click and the later header click land close enough in
        // time and position that `DoubleClickDetector`'s default folding
        // would otherwise merge them into a `UiEvent::DoubleClick` --
        // irrelevant to this test, which only drives plain clicks.
        driver.set_double_click_folding(false);
        driver.render();

        // Click the "dir" row (the only row painted so far) to
        // click-focus the extra -- a keyboard Space needs engine focus on
        // it first, same as any other focusable primitive.
        driver.click(10.0, 1.5);
        driver.render();
        assert_eq!(
            driver.backend().focus_tree().focused_id(),
            Some(&WidgetId::new("tt")),
            "clicking the row should click-focus the tree-table-controller extra"
        );

        // Space on the selected lazy branch expands it -- no loaded
        // children yet, so this requests them exactly once.
        driver.type_char(' ');
        driver.render();
        {
            let dispatched = &driver.app().sink.borrow().tree_table_dispatched;
            let requests = dispatched
                .iter()
                .filter(|(_, ev)| {
                    matches!(
                        ev,
                        TreeTableControllerWireEvent::ChildrenRequested { id } if id == "dir"
                    )
                })
                .count();
            assert_eq!(
                requests, 1,
                "expanding a lazy branch should request its children exactly once: {:?}",
                dispatched
            );
        }

        // Answer the request: fills "dir"'s level with two children, in
        // an intentionally unsorted order -- through the same wire-node
        // decode a `"tree_table_controller"` extra's own `nodes` use.
        let children = crate::host::tree_table_controller_nodes_from_wire(vec![
            TreeTableControllerNode {
                id: "zebra".to_string(),
                cells: vec![StyledText::plain("zebra")],
                icon: None,
                has_children: false,
                children: None,
            },
            TreeTableControllerNode {
                id: "apple".to_string(),
                cells: vec![StyledText::plain("apple")],
                icon: None,
                has_children: false,
                children: None,
            },
        ]);
        driver
            .app()
            .engine
            .set_tree_table_children(WidgetId::new("tt"), "dir", children);
        driver.render();

        // Down from "dir" selects its first (still unsorted) child,
        // "zebra" -- picked deliberately so the later "selection survives
        // the sort" assertion is a genuine id-keyed survival, not a lucky
        // positional coincidence.
        driver.press_named(NamedKey::Down);
        driver.render();
        {
            let dispatched = &driver.app().sink.borrow().tree_table_dispatched;
            assert!(
                dispatched.iter().any(|(_, ev)| matches!(
                    ev,
                    TreeTableControllerWireEvent::RowSelected { id } if id == "zebra"
                )),
                "Down from \"dir\" should select its first (unsorted) child \"zebra\": {:?}",
                dispatched
            );
        }
        driver.app().sink.borrow_mut().tree_table_dispatched.clear();

        // A header click sorts every loaded level's children ascending --
        // "zebra" moves from index 0 to index 1 among "dir"'s children,
        // but must stay selected by id, not by its old position.
        driver.click(10.0, 0.5);
        driver.render();
        {
            let dispatched = &driver.app().sink.borrow().tree_table_dispatched;
            assert!(
                dispatched.iter().any(|(_, ev)| matches!(
                    ev,
                    TreeTableControllerWireEvent::SortChanged {
                        col: 0,
                        ascending: true
                    }
                )),
                "a header click should sort ascending by column 0: {:?}",
                dispatched
            );
        }

        let controllers = driver.app().engine.tree_table_controllers.borrow();
        let controller = controllers.get(&WidgetId::new("tt")).unwrap();
        assert_eq!(
            controller.selected_id(),
            Some(&"zebra".to_string()),
            "selection should survive the sort by node id"
        );
        let table = controller.flatten(vec![Column {
            title: "Name".into(),
            width: ColumnWidth::Flex(1.0),
            align: ColumnAlign::Left,
        }]);
        assert_eq!(
            table
                .table
                .rows
                .iter()
                .map(|r| r.cells[0].spans[0].text.clone())
                .collect::<Vec<_>>(),
            vec!["dir".to_string(), "apple".to_string(), "zebra".to_string()],
            "children should sort ascending under their parent, not flatten into one list"
        );
        assert_eq!(
            table.table.selected_idx,
            Some(2),
            "\"zebra\" is now at flattened index 2 after the sort"
        );
    }

    /// A letter key is the app's shortcut while a tree-table controller
    /// has focus: the controller claims only its navigation keys, so the
    /// key falls through to the app's own key dispatch.
    #[test]
    fn tree_table_controller_leaves_letter_keys_to_the_app() {
        let logic = TreeTableControllerLogic {
            descriptor: lazy_root_descriptor(),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();
        driver.click(10.0, 1.5);
        driver.render();
        assert_eq!(
            driver.backend().focus_tree().focused_id(),
            Some(&WidgetId::new("tt"))
        );
        driver.app().sink.borrow_mut().dispatched.clear();
        driver.app().sink.borrow_mut().tree_table_dispatched.clear();

        // "d" would type-ahead to the "dir" row.
        driver.type_char('d');
        driver.render();
        let sink = driver.app().sink.borrow();
        assert!(
            sink.dispatched.iter().any(|(ev, _)| matches!(
                ev,
                UiEvent::KeyPressed {
                    key: Key::Char('d'),
                    ..
                }
            )),
            "the letter should reach the app's key dispatch: {:?}",
            sink.dispatched
        );
        assert!(
            sink.tree_table_dispatched.is_empty(),
            "the controller should not claim the letter: {:?}",
            sink.tree_table_dispatched
        );
    }

    /// `UiEvent::DoubleClick` — the event `DoubleClickDetector` folds a
    /// real second close-together `MouseDown` into, with default folding
    /// left on (`driver.double_click` delivers the folded shape directly,
    /// deterministically) — must reach the cached `TreeTableController`
    /// the same way a plain `MouseDown` does, not fall through unclaimed.
    /// A dropped `DoubleClick` here would mean `RowActivated`'s mouse half
    /// never fires and a rapid second click near the first is silently
    /// lost.
    #[test]
    fn tree_table_controller_double_click_activates_row() {
        let logic = TreeTableControllerLogic {
            descriptor: lazy_root_descriptor(),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();

        driver.double_click(10.0, 1.5);
        driver.render();

        assert_eq!(
            driver.backend().focus_tree().focused_id(),
            Some(&WidgetId::new("tt")),
            "a DoubleClick claimed by the tree-table-controller should also click-focus it"
        );
        let dispatched = &driver.app().sink.borrow().tree_table_dispatched;
        assert!(
            dispatched.iter().any(|(_, ev)| matches!(
                ev,
                TreeTableControllerWireEvent::RowActivated { id } if id == "dir"
            )),
            "a DoubleClick on the row should activate it: {:?}",
            dispatched
        );
    }

    /// Mirrors [`TreeTableControllerLogic`], for the generic
    /// [`crate::extra_paint::WIDGET_KINDS`] seam's `"cell_grid"` member
    /// instead: unlike that cached-controller logic, there is no engine-
    /// owned state to seed once and forget — `self.grid` is the single
    /// source of truth a real host would keep, and [`Self::apply`]
    /// round-trips every dispatched [`CellGridEvent`] back onto it after
    /// each `handle` call, the same "host owns the state, the widget only
    /// ever reports what changed" contract [`CellGridEvent::Navigated`]'s
    /// own doc describes.
    struct CellGridLogic {
        grid: RefCell<CellGrid>,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl CellGridLogic {
        /// Applies one dispatched [`CellGridEvent`] onto [`Self::grid`] —
        /// idempotent and order-preserving, so replaying the same event
        /// twice (or the whole history every `handle` call, rather than
        /// only the newest slice) converges to the same state either way.
        fn apply(&self, event: &CellGridEvent) {
            let mut grid = self.grid.borrow_mut();
            match event {
                CellGridEvent::Navigated {
                    cursor,
                    selection_anchor,
                    row_scroll,
                    col_scroll,
                } => {
                    grid.cursor = *cursor;
                    grid.selection_anchor = *selection_anchor;
                    grid.row_scroll = *row_scroll;
                    grid.col_scroll = *col_scroll;
                }
                CellGridEvent::CellEditStarted { edit, .. }
                | CellGridEvent::CellEditChanged { edit, .. } => {
                    grid.editing = Some(edit.clone());
                }
                CellGridEvent::CellEditCommitted { .. }
                | CellGridEvent::CellEditCancelled { .. } => {
                    grid.editing = None;
                }
                // `CellEditRequested` names no new state of its own to
                // apply (a host picks its own seed text for the editor,
                // the module doc's "no cell content to seed itself with"
                // gap), and `CellsRequested` is answered by swapping in a
                // new `cells`/`loaded_row_start`/`loaded_col_start`
                // block, which a test applies directly rather than
                // through this generic round-trip.
                _ => {}
            }
        }
    }

    impl AppLogic for CellGridLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let area = viewport_rect(backend);
            let grid = self.grid.borrow().clone();
            let id = grid.id.as_str().to_string();
            let content = LayoutNode::Leaf(LayoutLeaf {
                id,
                descriptor: LeafDescriptor::Widget {
                    kind: "cell_grid",
                    widget: Rc::new(grid),
                },
                scroll_px: None,
            });
            let mut hits = Vec::new();
            paint_layout(backend, area, &content, &mut hits);
            render_overlays(&self.engine, backend, hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            let reaction = self
                .engine
                .handle(event, backend, &mut *self.sink.borrow_mut());
            let id = self.grid.borrow().id;
            let events: Vec<CellGridEvent> = self
                .sink
                .borrow()
                .dispatched
                .iter()
                .filter_map(|(ev, _)| match ev {
                    UiEvent::CellGrid(ev_id, cg_ev) if *ev_id == id => Some(cg_ev.clone()),
                    _ => None,
                })
                .collect();
            for ev in &events {
                self.apply(ev);
            }
            reaction
        }
    }

    /// A 20x5 sheet with only 2 loaded rows — deliberately fewer than a
    /// 10-cell-tall viewport needs, so the very first routed event
    /// exercises [`InteractionEngine::dyn_widget_cells_requested`] without
    /// any scroll having happened yet. `cells[r][c]` is `"r{r}c{c}"`, a
    /// value every test can assert against unambiguously by position.
    fn small_window_cell_grid() -> CellGrid {
        let cells = (0..2)
            .map(|r| (0..5).map(|c| Cell::plain(format!("r{r}c{c}"))).collect())
            .collect();
        let mut grid = CellGrid::new(WidgetId::new("cg"));
        grid.row_count = 20;
        grid.col_count = 5;
        grid.default_col_width = 8.0;
        grid.cells = cells;
        grid.has_focus = true;
        grid
    }

    /// A click inside the `"cell_grid"` leaf both click-focuses it (so the
    /// next keystroke reaches it) and reports a `Navigated` event through
    /// the generic [`EventSink::dispatch`] — the same "claim -> focus ->
    /// dispatch" shape the tree-table-controller claim loop exercises in
    /// [`tree_table_controller_double_click_activates_row`].
    #[test]
    fn cell_grid_click_moves_cursor_and_focuses() {
        let logic = CellGridLogic {
            grid: RefCell::new(small_window_cell_grid()),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();

        // Row headers + col headers each take one leading row/column at
        // `default_row_height`/`default_col_width` scale -- (1, 1) in
        // cell units lands inside the body's first data cell, (0, 0).
        driver.click(9.0, 1.5);
        driver.render();

        assert_eq!(
            driver.backend().focus_tree().focused_id(),
            Some(&WidgetId::new("cg")),
            "clicking a cell should click-focus the cell_grid leaf"
        );
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(ev, _)| matches!(
                ev,
                UiEvent::CellGrid(
                    id,
                    CellGridEvent::Navigated {
                        cursor: (0, 0),
                        ..
                    }
                ) if *id == WidgetId::new("cg")
            )),
            "clicking the first body cell should move the cursor to (0, 0): {:?}",
            dispatched
        );
    }

    /// A left-button drag across more than one cell reports a `Navigated`
    /// event with the extended selection anchor — the generic captured-
    /// drag routing ([`InteractionEngine::dispatch_dyn_widget_captured_move`])
    /// routes through the full [`InteractionEngine::dyn_widget_dispatch`]
    /// rather than a bare `Consumed` check specifically so this reaches
    /// `sink` (see that method's own doc).
    #[test]
    fn cell_grid_drag_extends_selection() {
        let logic = CellGridLogic {
            grid: RefCell::new(small_window_cell_grid()),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();

        driver.drag(9.0, 1.5, 17.0, 2.5);
        driver.render();

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(ev, _)| matches!(
                ev,
                UiEvent::CellGrid(
                    id,
                    CellGridEvent::Navigated {
                        cursor: (1, 1),
                        selection_anchor: Some((0, 0)),
                        ..
                    }
                ) if *id == WidgetId::new("cg")
            )),
            "dragging from (0, 0) to (1, 1) should report that selection: {:?}",
            dispatched
        );
    }

    /// The loaded `cells` window (2 rows) doesn't cover what a 10-cell-tall
    /// viewport needs — [`InteractionEngine::dyn_widget_cells_requested`]
    /// catches this on the very first routed event, with no scroll
    /// required to trigger it.
    #[test]
    fn cell_grid_requests_cells_when_window_too_small() {
        let logic = CellGridLogic {
            grid: RefCell::new(small_window_cell_grid()),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();

        driver.click(9.0, 1.5);
        driver.render();

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(ev, _)| matches!(
                ev,
                UiEvent::CellGrid(
                    id,
                    CellGridEvent::CellsRequested {
                        row_start: 0,
                        col_start: 0,
                        ..
                    }
                ) if *id == WidgetId::new("cg")
            )),
            "a 2-row loaded window under a taller viewport should request more rows: {:?}",
            dispatched
        );
    }

    /// Mirrors [`small_window_cell_grid`] but with `frozen_rows: 1` and
    /// `cells`/`loaded_row_start` already covering the entire sheet —
    /// regression coverage for [`quadraui::CellGrid::cells_requested`]
    /// needing its own `row_count`/`col_count` set: with those left at
    /// `0`, [`CellGrid::cells_window`]'s `frozen_rows =
    /// self.frozen_rows.min(self.row_count)` collapsed to `0` and
    /// `row_start = self.loaded_row_start.clamp(frozen_rows, self.row_count)`
    /// collapsed to always-`0`, so a grid with any frozen rows reported
    /// `CellsRequested` forever even when fully loaded.
    fn frozen_rows_cell_grid() -> CellGrid {
        let cells = (0..20)
            .map(|r| (0..5).map(|c| Cell::plain(format!("r{r}c{c}"))).collect())
            .collect();
        let mut grid = CellGrid::new(WidgetId::new("cg"));
        grid.row_count = 20;
        grid.col_count = 5;
        grid.frozen_rows = 1;
        grid.default_col_width = 8.0;
        grid.cells = cells;
        grid.loaded_row_start = 1;
        grid.has_focus = true;
        grid
    }

    #[test]
    fn cell_grid_frozen_rows_fully_loaded_does_not_request_cells() {
        let logic = CellGridLogic {
            grid: RefCell::new(frozen_rows_cell_grid()),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();

        driver.click(9.0, 1.5);
        driver.render();

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            !dispatched.iter().any(|(ev, _)| matches!(
                ev,
                UiEvent::CellGrid(id, CellGridEvent::CellsRequested { .. })
                    if *id == WidgetId::new("cg")
            )),
            "a `frozen_rows > 0` grid whose `cells`/`loaded_row_start` already cover the \
             whole sheet must not request more cells: {:?}",
            dispatched
        );
    }

    /// After a host answers an initial `CellsRequested` by loading exactly
    /// the named block, a later event that moves the cursor back onto the
    /// already-loaded window (same screen position, now mapped to the
    /// scrolled block) must not see the request repeat — the regression
    /// this covers is [`CellGrid::cells_window`]'s `row_start`/`col_start`
    /// landing on `0` regardless of the `loaded_row_start`/`loaded_col_start`
    /// the host just answered with, which kept re-requesting the same
    /// block forever.
    #[test]
    fn cell_grid_cells_requested_does_not_repeat_after_host_loads_window() {
        let logic = CellGridLogic {
            grid: RefCell::new(small_window_cell_grid()),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        // This test clicks the same screen position twice, with no real
        // wall-clock delay between them — without this, `TuiBackend`'s
        // own `DoubleClickDetector` would fold the second click into a
        // `DoubleClick` instead of delivering the plain `MouseDown` this
        // test means to send.
        driver.set_double_click_folding(false);
        driver.render();
        driver.click(9.0, 1.5);
        driver.render();
        driver.app().sink.borrow_mut().dispatched.clear();

        // Scroll past the 2 loaded rows so the visible window's absolute
        // start moves away from 0.
        driver.press_named(NamedKey::PageDown);
        driver.render();

        let (row_start, col_start, rows, cols) = {
            let dispatched = driver.app().sink.borrow().dispatched.clone();
            dispatched
                .iter()
                .find_map(|(ev, _)| match ev {
                    UiEvent::CellGrid(
                        id,
                        CellGridEvent::CellsRequested {
                            row_start,
                            col_start,
                            rows,
                            cols,
                        },
                    ) if *id == WidgetId::new("cg") => Some((*row_start, *col_start, *rows, *cols)),
                    _ => None,
                })
                .expect("paging down past the 2-row loaded window should request a new block")
        };
        assert!(
            row_start > 0,
            "paging down past a 2-row loaded window should request a block starting \
             after row 0, got row_start={row_start}"
        );

        // The host answers with exactly the named block.
        let cells = (0..rows)
            .map(|r| {
                (0..cols)
                    .map(|c| Cell::plain(format!("r{}c{}", row_start + r, col_start + c)))
                    .collect()
            })
            .collect();
        {
            let mut grid = driver.app().grid.borrow_mut();
            grid.cells = cells;
            grid.loaded_row_start = row_start;
            grid.loaded_col_start = col_start;
        }
        driver.app().sink.borrow_mut().dispatched.clear();
        driver.render();

        // Clicking the same screen position again lands on the (now
        // scrolled) already-loaded cell -- a cursor move with no further
        // scroll change.
        driver.click(9.0, 1.5);
        driver.render();

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            !dispatched.iter().any(|(ev, _)| matches!(
                ev,
                UiEvent::CellGrid(id, CellGridEvent::CellsRequested { .. })
                    if *id == WidgetId::new("cg")
            )),
            "once the host has loaded exactly the requested block, a follow-up event \
             should not repeat the request: {:?}",
            dispatched
        );
    }

    /// A terminal bracketed paste with no active edit routes through
    /// [`RoutedEvent::Text`], the same shape committed IME text arrives
    /// as — [`CellGrid::handle`][quadraui::interactive::Interactive::handle]
    /// treats it as the start of a fresh edit seeded with the whole
    /// pasted string, same as typing a bare character with no active
    /// edit does. Unlike the deleted [`CellGridController`]-based
    /// `PasteRequested` wire event, this never spreads a multi-line/TSV
    /// paste across more than the cursor cell — see
    /// [`InteractionEngine::dispatch_dyn_widget_key`]'s own doc.
    #[test]
    fn cell_grid_paste_with_no_active_edit_starts_a_single_cell_edit() {
        let logic = CellGridLogic {
            grid: RefCell::new(small_window_cell_grid()),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();
        driver.click(9.0, 1.5);
        driver.render();
        driver.app().sink.borrow_mut().dispatched.clear();

        driver.dispatch(UiEvent::ClipboardPaste("pasted\ttext".to_string()));
        driver.render();

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(ev, _)| matches!(
                ev,
                UiEvent::CellGrid(
                    id,
                    CellGridEvent::CellEditStarted {
                        row: 0,
                        col: 0,
                        edit,
                    }
                ) if *id == WidgetId::new("cg") && edit.text == "pasted\ttext"
            )),
            "a bracketed paste with no active edit should start a single-cell edit \
             seeded with the whole pasted text: {:?}",
            dispatched
        );
    }

    /// Typing a character with no active edit starts one; Enter commits
    /// it and reports `CellEditCommitted` with the typed text — `CharTyped`
    /// reaches the focused-key block for exactly this reason (see that
    /// block's own doc).
    #[test]
    fn cell_grid_typed_edit_commits_on_enter() {
        let logic = CellGridLogic {
            grid: RefCell::new(small_window_cell_grid()),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();
        driver.click(9.0, 1.5);
        driver.render();

        driver.type_char('x');
        driver.render();
        driver.press_named(NamedKey::Enter);
        driver.render();

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(ev, _)| matches!(
                ev,
                UiEvent::CellGrid(
                    id,
                    CellGridEvent::CellEditCommitted {
                        row: 0,
                        col: 0,
                        text,
                    }
                ) if *id == WidgetId::new("cg") && text == "x"
            )),
            "typing \"x\" then Enter should commit a CellEditCommitted with that text: {:?}",
            dispatched
        );
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
            backend.focus_tree().focused_id().copied(),
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
    /// `preprocess_event` sees `app.extra_tab_stops()` return the non-empty
    /// `[before, after]` (the form's own stop excluded while it holds
    /// focus), calls `FocusTree::sync_tab_order` on it, which clears
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
                backend.focus_tree().focused_id().copied(),
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
                backend.focus_tree().focused_id().copied(),
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
            backend.focus_tree().focused_id().copied(),
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
                backend.focus_tree().focused_id().copied(),
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
            backend.focus_tree().focused_id().copied(),
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
    /// dispatch) followed by the generic `"button"`-kind
    /// [`HitDetail::WidgetBody`] seam's own
    /// [`quadraui::UiEvent::ButtonActivated`] — [`crate::engine::
    /// InteractionEngine::dispatch_dyn_widget_click`]'s replacement for
    /// the raw `MouseDown`, the same shape a `"switch"`/`"radio_group"`
    /// leaf click already produces. Proves the orchestration itself (not
    /// just paint/hit-test) is usable with no host-language binding in
    /// scope.
    #[test]
    fn click_on_a_button_focuses_it_then_dispatches_button_activated() {
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
                    crate::hit::PaintedHit::Widget { id, area, widget }
                        if *id == WidgetId::new("go") && widget.as_any().is::<Button>() =>
                    {
                        Some(*area)
                    }
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
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::FocusChanged(Some(id)) if *id == WidgetId::new("go")
            )),
            "clicking the button must click-focus it: {:?}",
            dispatched
        );
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::ButtonActivated { widget } if *widget == WidgetId::new("go")
            )),
            "expected a ButtonActivated event for the clicked button: {:?}",
            dispatched
        );
    }

    /// Mirrors [`MinimalLogic`], painting one `"command_center"` widget
    /// leaf instead of a `Button`.
    struct CommandCenterLogic {
        cc: quadraui::CommandCenter,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for CommandCenterLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let extra = HashMap::new();
            let mut widgets: HashMap<String, Rc<dyn DynWidget>> = HashMap::new();
            widgets.insert("command_center".to_string(), Rc::new(self.cc.clone()));
            draw_bands(
                backend,
                RenderRefs {
                    text_display: None,
                    list: None,
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                    widgets: &widgets,
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
            );
            self.engine.render_overlays(backend);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Paints one `"palette"` widget leaf over the whole viewport through
    /// the layout tree, the shape a binding app's `{"row": [{"palette":
    /// ...}]}` render produces.
    struct PaletteLeafLogic {
        palette: quadraui::Palette,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl PaletteLeafLogic {
        fn new() -> Self {
            let row = |text: &str| quadraui::PaletteItem {
                text: StyledText::plain(text),
                detail: None,
                icon: None,
                match_positions: Vec::new(),
                depth: 0,
                expandable: false,
                expanded: false,
            };
            Self {
                palette: quadraui::Palette {
                    id: WidgetId::new("pal"),
                    title: "Commands".to_string(),
                    items: vec![row("alpha"), row("beta"), row("gamma")],
                    total_count: 3,
                    has_focus: true,
                    ..quadraui::Palette::default()
                },
                engine: InteractionEngine::new(),
                sink: RefCell::new(RecordingSink::default()),
            }
        }

        fn palette_events(&self) -> Vec<UiEvent> {
            self.sink
                .borrow()
                .dispatched
                .iter()
                .filter(|(event, _)| matches!(event, UiEvent::Palette(..)))
                .map(|(event, _)| event.clone())
                .collect()
        }
    }

    impl AppLogic for PaletteLeafLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let node = LayoutNode::Leaf(LayoutLeaf {
                id: "pal".to_string(),
                descriptor: LeafDescriptor::Widget {
                    kind: "palette",
                    widget: Rc::new(self.palette.clone()),
                },
                scroll_px: None,
            });
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// The binding-host leg of `Palette`'s click parity: a `MouseDown` on
    /// the third row of a `"palette"` leaf resolves through
    /// [`InteractionEngine::handle`] to the same
    /// `Palette(pal, SelectionChanged { idx: 2 })` the `TuiDriver` leg
    /// (`quadraui/tests/palette_form_event_parity.rs`) produces.
    #[test]
    fn click_on_a_palette_row_dispatches_the_selection_parity_event() {
        let mut driver = TuiDriver::new(PaletteLeafLogic::new(), 40, 14);
        driver.render();
        let (x, y) = quadraui::primitive::painted_sub_center(
            &WidgetId::new("pal"),
            quadraui::interactive::SubTarget(2),
        )
        .expect("the palette records each row it painted");
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
            modifiers: Modifiers::default(),
        });
        assert_eq!(
            driver.app().palette_events(),
            vec![UiEvent::Palette(
                WidgetId::new("pal"),
                quadraui::PaletteEvent::SelectionChanged { idx: 2 }
            )]
        );
    }

    /// The binding-host leg of `Palette`'s confirm parity: Enter on a
    /// focused `"palette"` leaf dispatches the same
    /// `Palette(pal, ItemConfirmed { idx: 0 })` the `TuiDriver` leg and
    /// the `a11y::route_widget_action` leg produce.
    #[test]
    fn enter_on_a_focused_palette_dispatches_the_confirm_parity_event() {
        let mut driver = TuiDriver::new(PaletteLeafLogic::new(), 40, 14);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            backend.set_focus(Some(WidgetId::new("pal")));
        }
        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        assert_eq!(
            driver.app().palette_events(),
            vec![UiEvent::Palette(
                WidgetId::new("pal"),
                quadraui::PaletteEvent::ItemConfirmed { idx: 0 }
            )]
        );
    }

    /// The binding-host leg of `Form`'s toggle parity: a `MouseDown` on a
    /// `"form"` leaf's focused toggle row dispatches the same
    /// `Form(settings, ToggleChanged { notify, true })` the `TuiDriver`
    /// leg (`quadraui/tests/palette_form_event_parity.rs`) and the
    /// `a11y::route_widget_action` leg produce.
    #[test]
    fn click_on_a_form_toggle_row_dispatches_the_toggle_parity_event() {
        use quadraui::{FieldKind, FormEvent};

        let mut form = uncontrolled_text_form("settings", &[]);
        form.fields = vec![quadraui::FormField {
            kind: FieldKind::Toggle { value: false },
            label: StyledText::plain("Notify"),
            ..text_input_field("notify")
        }];
        form.focused_field = Some(WidgetId::new("notify"));
        form.has_focus = true;
        let logic = FormLogic {
            form,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 14);
        driver.render();
        let (x, y) = driver.find("Notify").expect("the toggle row is painted");
        driver.click(x, y);
        let toggles: Vec<UiEvent> = driver
            .app()
            .sink
            .borrow()
            .dispatched
            .iter()
            .filter(|(event, _)| matches!(event, UiEvent::Form(_, FormEvent::ToggleChanged { .. })))
            .map(|(event, _)| event.clone())
            .collect();
        assert_eq!(
            toggles,
            vec![UiEvent::Form(
                WidgetId::new("settings"),
                FormEvent::ToggleChanged {
                    id: WidgetId::new("notify"),
                    value: true
                }
            )]
        );
    }

    /// Paints one widget leaf of `kind` over the whole viewport through the
    /// layout tree, the shape a binding app's `{"row": [{"<kind>": ...}]}`
    /// render produces.
    struct LayoutWidgetLeafLogic {
        id: &'static str,
        kind: &'static str,
        widget: Rc<dyn DynWidget>,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl LayoutWidgetLeafLogic {
        fn new(id: &'static str, kind: &'static str, widget: Rc<dyn DynWidget>) -> Self {
            Self {
                id,
                kind,
                widget,
                engine: InteractionEngine::new(),
                sink: RefCell::new(RecordingSink::default()),
            }
        }

        fn events(&self, keep: impl Fn(&UiEvent) -> bool) -> Vec<UiEvent> {
            self.sink
                .borrow()
                .dispatched
                .iter()
                .filter(|(event, _)| keep(event))
                .map(|(event, _)| event.clone())
                .collect()
        }
    }

    impl AppLogic for LayoutWidgetLeafLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let node = LayoutNode::Leaf(LayoutLeaf {
                id: self.id.to_string(),
                descriptor: LeafDescriptor::Widget {
                    kind: self.kind,
                    widget: self.widget.clone(),
                },
                scroll_px: None,
            });
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// The binding-host leg of `Editor`'s pointer parity: a press at text
    /// column 3 on view row 1 of a 20x6 `"editor"` leaf dispatches the same
    /// `Editor(ed, Pointer { BufferPos { 11, 3 }, 3, Down(Left) })` the
    /// `TuiDriver` leg (`quadraui/tests/editor_minimap_event_parity.rs`)
    /// produces.
    #[test]
    fn press_on_editor_text_dispatches_the_pointer_parity_event() {
        let line = |idx: usize, text: &str| quadraui::EditorLine {
            raw_text: text.to_string(),
            gutter_text: format!("{:>3} ", idx + 1),
            spans: vec![],
            line_idx: idx,
            is_current_line: false,
            is_fold_header: false,
            folded_line_count: 0,
            git_diff: None,
            diff_status: None,
            diagnostics: vec![],
            spell_errors: vec![],
            is_breakpoint: false,
            is_conditional_bp: false,
            is_dap_current: false,
            is_wrap_continuation: false,
            segment_col_offset: 0,
            annotation: None,
            ghost_suffix: None,
            is_ghost_continuation: false,
            indent_guides: vec![],
            colorcolumns: vec![],
        };
        let editor = quadraui::Editor::new(WidgetId::new("ed"), Rect::new(0.0, 0.0, 20.0, 6.0))
            .with_lines(vec![
                line(10, "fn main() {"),
                line(11, "    hello();"),
                line(12, "}"),
            ])
            .with_scroll_top(10)
            .with_total_lines(60)
            .with_gutter_char_width(4)
            .with_is_active(true);
        let logic = LayoutWidgetLeafLogic::new("ed", "editor", Rc::new(editor));
        let mut driver = TuiDriver::new(logic, 20, 6);
        driver.render();
        driver.click(4.0 + 3.0, 1.0);
        assert_eq!(
            driver.app().events(|e| matches!(e, UiEvent::Editor(..))),
            vec![UiEvent::Editor(
                WidgetId::new("ed"),
                quadraui::EditorEvent::Pointer {
                    hit: quadraui::EditorHit::BufferPos { line: 11, col: 3 },
                    col: 3,
                    kind: quadraui::interactive::PointerKind::Down(MouseButton::Left),
                    mods: Modifiers::default(),
                }
            )]
        );
    }

    /// The binding-host leg of `Minimap`'s seek parity: a press at the
    /// vertical middle of a 4x8 `"minimap"` leaf dispatches the same
    /// `Minimap(mm, Seek { fraction: 0.5 })` the `TuiDriver` leg
    /// (`quadraui/tests/editor_minimap_event_parity.rs`) produces.
    #[test]
    fn press_at_the_minimap_middle_dispatches_the_seek_parity_event() {
        let minimap = quadraui::Minimap {
            id: WidgetId::new("mm"),
            lines: (0..32)
                .map(|i| quadraui::MinimapLine {
                    text: "fn main() {}".to_string(),
                    line_idx: i,
                })
                .collect(),
            syntax_spans: vec![],
            visible_row_start: 0,
            visible_row_count: 8,
            total_buffer_lines: 32,
            scale: Default::default(),
            sizing: None,
        };
        let logic = LayoutWidgetLeafLogic::new("mm", "minimap", Rc::new(minimap));
        let mut driver = TuiDriver::new(logic, 4, 8);
        driver.render();
        driver.click(1.0, 4.0);
        assert_eq!(
            driver.app().events(|e| matches!(e, UiEvent::Minimap(..))),
            vec![UiEvent::Minimap(
                WidgetId::new("mm"),
                quadraui::MinimapEvent::Seek { fraction: 0.5 }
            )]
        );
    }

    /// The binding-host legs of the group-F event parity: each widget is a
    /// layout leaf filling the viewport, driven with the same gesture and
    /// expected to report the same events as the `TuiDriver` leg
    /// (`quadraui/tests/group_f_event_parity.rs`).
    mod group_f_event_parity {
        use super::*;
        use quadraui::CanvasEvent;

        fn leaf(
            id: &'static str,
            kind: &'static str,
            widget: Rc<dyn DynWidget>,
            cols: u16,
            rows: u16,
        ) -> TuiDriver<LayoutWidgetLeafLogic> {
            let mut driver =
                TuiDriver::new(LayoutWidgetLeafLogic::new(id, kind, widget), cols, rows);
            driver.render();
            driver
        }

        fn is_form(e: &UiEvent) -> bool {
            matches!(e, UiEvent::Form(..))
        }

        #[test]
        fn a_click_on_a_checkbox_leaf_matches_the_parity_event() {
            let id = WidgetId::new("cb");
            let mut driver = leaf("cb", "checkbox", Rc::new(quadraui::Checkbox::new(id)), 6, 1);
            driver.click(1.0, 0.0);
            assert_eq!(
                driver.app().events(is_form),
                vec![UiEvent::Form(
                    id,
                    quadraui::FormEvent::ToggleChanged { id, value: true }
                )]
            );
        }

        #[test]
        fn a_click_on_a_select_leaf_matches_the_parity_event() {
            let id = WidgetId::new("sel");
            let select =
                quadraui::Select::new(id, vec!["One".into(), "Two".into(), "Three".into()]);
            let mut driver = leaf("sel", "select", Rc::new(select), 16, 1);
            driver.click(2.0, 0.0);
            assert_eq!(
                driver.app().events(is_form),
                vec![UiEvent::Form(
                    id,
                    quadraui::FormEvent::DropdownChanged {
                        id,
                        selected_idx: 1
                    }
                )]
            );
        }

        #[test]
        fn a_click_at_the_slider_leaf_track_end_matches_the_parity_event() {
            let id = WidgetId::new("sl");
            let slider = quadraui::Slider::new(id, 0.0, 0.0, 10.0);
            let mut driver = leaf("sl", "slider", Rc::new(slider), 24, 1);
            driver.click(23.0, 0.0);
            assert_eq!(
                driver.app().events(is_form),
                vec![UiEvent::Form(
                    id,
                    quadraui::FormEvent::SliderChanged { id, value: 10.0 }
                )]
            );
        }

        #[test]
        fn hover_and_a_click_on_a_canvas_leaf_region_match_the_parity_events() {
            let id = WidgetId::new("canvas");
            let canvas = quadraui::Canvas {
                id,
                ops: vec![],
                regions: vec![quadraui::CanvasRegion {
                    id: WidgetId::new("region:a"),
                    rect: Rect::new(0.0, 0.0, 10.0, 1.0),
                    cursor: None,
                }],
                focusable: false,
            };
            let mut driver = leaf("canvas", "canvas", Rc::new(canvas), 20, 4);
            driver.mouse_move(1.0, 0.0);
            driver.mouse_move(15.0, 3.0);
            driver.mouse_move(30.0, 10.0);
            driver.click(1.0, 0.0);
            assert_eq!(
                driver.app().events(|e| matches!(e, UiEvent::Canvas(..))),
                vec![
                    UiEvent::Canvas(
                        id,
                        CanvasEvent::RegionHovered {
                            region: Some("region:a".into())
                        }
                    ),
                    UiEvent::Canvas(id, CanvasEvent::RegionHovered { region: None }),
                    UiEvent::Canvas(
                        id,
                        CanvasEvent::RegionClicked {
                            region: WidgetId::new("region:a"),
                            button: MouseButton::Left,
                            modifiers: Modifiers::default(),
                            x: 1.0,
                            y: 0.0,
                        }
                    ),
                ]
            );
        }

        #[test]
        fn hovering_a_chart_leaf_point_matches_the_parity_event() {
            let chart = quadraui::Chart {
                id: WidgetId::new("chart"),
                kind: quadraui::ChartKind::Bar,
                series: vec![quadraui::Series {
                    label: "load".into(),
                    data: vec![1.0, 3.0, 2.0, 4.0],
                    color: None,
                    fill: false,
                }],
                x_label: None,
                y_label: None,
                y_range: Some((0.0, 4.0)),
                x_range: None,
                show_legend: true,
                y_ticks: Some(0),
                x_ticks: None,
                show_grid: false,
                hovered_point: None,
                crosshair_x: None,
            };
            let layout = chart.layout_in(Rect::new(0.0, 0.0, 24.0, 8.0), 1.0, 1.0);
            let &(_, _, x, y) = layout
                .data_point_positions
                .iter()
                .find(|&&(si, di, _, _)| si == 0 && di == 3)
                .expect("point laid out");
            let mut driver = leaf("chart", "chart", Rc::new(chart), 24, 8);
            driver.mouse_move(x, y);
            let points: Vec<Option<(usize, usize)>> = driver
                .app()
                .events(|e| matches!(e, UiEvent::Chart(..)))
                .into_iter()
                .filter_map(|e| match e {
                    UiEvent::Chart(_, quadraui::ChartEvent::DataPointHovered { point, .. }) => {
                        Some(point)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(points, vec![Some((0, 3))]);
        }

        #[test]
        fn moving_across_a_chart_leaf_reports_each_point_it_reaches() {
            let chart = quadraui::Chart {
                id: WidgetId::new("chart"),
                kind: quadraui::ChartKind::Bar,
                series: vec![quadraui::Series {
                    label: "load".into(),
                    data: vec![1.0, 3.0, 2.0, 4.0],
                    color: None,
                    fill: false,
                }],
                x_label: None,
                y_label: None,
                y_range: Some((0.0, 4.0)),
                x_range: None,
                show_legend: true,
                y_ticks: Some(0),
                x_ticks: None,
                show_grid: false,
                hovered_point: None,
                crosshair_x: None,
            };
            let layout = chart.layout_in(Rect::new(0.0, 0.0, 24.0, 8.0), 1.0, 1.0);
            let point = |di: usize| {
                let &(_, _, x, y) = layout
                    .data_point_positions
                    .iter()
                    .find(|&&(si, d, _, _)| si == 0 && d == di)
                    .expect("point laid out");
                (x, y)
            };
            let mut driver = leaf("chart", "chart", Rc::new(chart), 24, 8);
            // The descriptor's `hovered_point` stays `None`, so the chart
            // reports the point under the pointer on every move it gets.
            for di in [3, 1, 2] {
                let (x, y) = point(di);
                driver.mouse_move(x, y);
            }
            let points: Vec<Option<(usize, usize)>> = driver
                .app()
                .events(|e| matches!(e, UiEvent::Chart(..)))
                .into_iter()
                .filter_map(|e| match e {
                    UiEvent::Chart(_, quadraui::ChartEvent::DataPointHovered { point, .. }) => {
                        Some(point)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(points, vec![Some((0, 3)), Some((0, 1)), Some((0, 2))]);
        }

        #[test]
        fn moving_between_canvas_leaf_regions_reports_each_change_once() {
            let id = WidgetId::new("canvas");
            let region = |name: &str, x: f32| quadraui::CanvasRegion {
                id: WidgetId::new(name),
                rect: Rect::new(x, 0.0, 4.0, 1.0),
                cursor: None,
            };
            let canvas = quadraui::Canvas {
                id,
                ops: vec![],
                regions: vec![region("region:a", 0.0), region("region:b", 4.0)],
                focusable: false,
            };
            let mut driver = leaf("canvas", "canvas", Rc::new(canvas), 20, 4);
            driver.mouse_move(1.0, 0.0);
            driver.mouse_move(5.0, 0.0);
            driver.mouse_move(12.0, 2.0);
            driver.mouse_move(14.0, 3.0);
            driver.mouse_move(2.0, 0.0);
            let hovered = |name: Option<&str>| {
                UiEvent::Canvas(
                    id,
                    CanvasEvent::RegionHovered {
                        region: name.map(str::to_string),
                    },
                )
            };
            assert_eq!(
                driver.app().events(|e| matches!(e, UiEvent::Canvas(..))),
                vec![
                    hovered(Some("region:a")),
                    hovered(Some("region:b")),
                    hovered(None),
                    hovered(Some("region:a")),
                ]
            );
        }

        fn card(id: &str, title: &str) -> quadraui::BoardCard {
            quadraui::BoardCard {
                id: WidgetId::new(id),
                title: title.to_string(),
                labels: vec![],
                badges: vec![quadraui::CardBadge {
                    label: "T".into(),
                    status: quadraui::BadgeStatus::Passed,
                }],
                hint: None,
            }
        }

        #[test]
        fn a_click_on_a_board_leaf_card_matches_the_parity_event() {
            let board = quadraui::BoardModel {
                id: WidgetId::new("board"),
                columns: vec![
                    quadraui::BoardColumn {
                        id: WidgetId::new("col:todo"),
                        title: "Todo".to_string(),
                        cards: vec![card("card:1", "First card"), card("card:2", "Second card")],
                        scroll_offset: 0,
                    },
                    quadraui::BoardColumn {
                        id: WidgetId::new("col:done"),
                        title: "Done".to_string(),
                        cards: vec![card("card:3", "Third card")],
                        scroll_offset: 0,
                    },
                ],
                selected_card_id: None,
                col_scroll_offset: 0,
            };
            let mut driver = leaf("board", "board", Rc::new(board), 50, 13);
            let (x, y) = driver.find("Third card").expect("card painted");
            driver.click(x, y);
            assert_eq!(
                driver.app().events(|e| matches!(e, UiEvent::Board(..))),
                vec![UiEvent::Board(
                    WidgetId::new("board"),
                    quadraui::BoardAction::SelectCard(WidgetId::new("card:3"))
                )]
            );
        }

        #[test]
        fn a_click_on_a_pipeline_leaf_stage_matches_the_parity_event() {
            let stage = |label: &str, status, action: Option<&str>| quadraui::PipelineStage {
                label: label.to_string(),
                status,
                action: action.map(str::to_string),
            };
            let view = quadraui::PipelineView {
                id: WidgetId::new("pipe"),
                stages: vec![
                    stage("Build", quadraui::StageStatus::Done, None),
                    stage("Test", quadraui::StageStatus::Active, Some("Retry")),
                    stage("Ship", quadraui::StageStatus::Pending, None),
                ],
                focused_stage: Some(0),
            };
            let mut driver = leaf("pipe", "pipeline_view", Rc::new(view), 60, 7);
            let (x, y) = driver.find("Ship").expect("stage painted");
            driver.click(x, y);
            assert_eq!(
                driver
                    .app()
                    .events(|e| matches!(e, UiEvent::PipelineView(..))),
                vec![UiEvent::PipelineView(
                    WidgetId::new("pipe"),
                    quadraui::PipelineEvent::StageSelected { index: 2 }
                )]
            );
        }
    }

    /// An `"image"` leaf decodes through the generic widget registry and
    /// paints its fallback text on a terminal with no graphics channel.
    #[test]
    fn an_image_leaf_paints_its_fallback_text() {
        let image = quadraui::Image {
            id: WidgetId::new("img"),
            source: quadraui::ImageSource::Bytes(Vec::new()),
            intrinsic_size: Some((16, 16)),
            fit: quadraui::ImageFit::Contain,
            fallback_text: "[LOGO]".to_string(),
        };
        let logic = LayoutWidgetLeafLogic::new("img", "image", Rc::new(image));
        let mut driver = TuiDriver::new(logic, 20, 3);
        driver.render();
        assert!(
            driver.find("[LOGO]").is_some(),
            "the fallback text paints:\n{}",
            driver.screen()
        );
    }

    /// Mirrors [`MinimalLogic`], painting one `"terminal"` widget leaf.
    struct TerminalLogic {
        term: quadraui::Terminal,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl TerminalLogic {
        fn new(rows: usize, cols: usize) -> Self {
            let cell = quadraui::TerminalCell {
                text: "x".into(),
                fg: quadraui::Color::rgb(200, 200, 200),
                bg: quadraui::Color::rgb(20, 20, 20),
                bold: false,
                italic: false,
                underline: false,
                dim: false,
                selected: false,
                is_cursor: false,
                is_find_match: false,
                is_find_active: false,
                cursor_shape: quadraui::TerminalCursorShape::Block,
                cursor_blinking: false,
            };
            Self {
                term: quadraui::Terminal {
                    id: WidgetId::new("term"),
                    cells: vec![vec![cell; cols]; rows],
                    scrollbar: None,
                },
                engine: InteractionEngine::new(),
                sink: RefCell::new(RecordingSink::default()),
            }
        }

        fn terminal_area(&self) -> Rect {
            self.engine
                .hit_map()
                .iter()
                .find_map(|hit| match hit {
                    PaintedHit::Widget { id, area, .. } if *id == self.term.id => Some(*area),
                    _ => None,
                })
                .expect("the terminal records a widget hit for its painted area")
        }
    }

    impl AppLogic for TerminalLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let extra = HashMap::new();
            let mut widgets: HashMap<String, Rc<dyn DynWidget>> = HashMap::new();
            widgets.insert("terminal".to_string(), Rc::new(self.term.clone()));
            draw_bands(
                backend,
                RenderRefs {
                    text_display: None,
                    list: None,
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                    widgets: &widgets,
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
            );
            self.engine.render_overlays(backend);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// The binding-host leg of `Terminal`'s event parity: one upward
    /// wheel notch over a `"terminal"` leaf resolves through
    /// [`InteractionEngine::handle`] to the same `Scroll { delta: -1 }`
    /// the `TuiDriver` leg (`quadraui/tests/widget_contract_terminal.rs`)
    /// and the `a11y::route_widget_action` leg produce.
    #[test]
    fn wheel_over_a_terminal_dispatches_terminal_scroll() {
        let mut driver = TuiDriver::new(TerminalLogic::new(5, 10), 40, 20);
        let area = driver.app().terminal_area();

        driver.dispatch(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta::new(0.0, 1.0),
            position: Point::new(area.x + 1.0, area.y + 1.0),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| *event
                == UiEvent::Terminal(
                    WidgetId::new("term"),
                    quadraui::TerminalEvent::Scroll { delta: -1 }
                )),
            "expected Terminal Scroll {{ delta: -1 }}: {dispatched:?}"
        );
    }

    /// On a pixel backend whose editor line height differs from the
    /// chrome row height, a click on a terminal row resolves against the
    /// row geometry the terminal was painted with: the click path lays
    /// the widget out with the same editor line height, measurer and
    /// style `draw_dyn` uses.
    #[test]
    fn a_terminal_click_on_a_pixel_backend_hits_the_painted_row() {
        let mut backend = quadraui::testing::RecordingBackend {
            viewport: quadraui::Viewport {
                width: 400.0,
                height: 400.0,
                scale: 1.0,
            },
            line_height: 16.0,
            char_width: 8.0,
            chrome_line_height: Some(16.0),
            ..Default::default()
        };
        backend.current_style.row_height = 22.0;
        let mut logic = TerminalLogic::new(10, 20);
        logic.render(&mut backend, ());
        let area = logic.terminal_area();

        // The band is three chrome lines tall: painted rows are 16px, so
        // y = 40 is the middle of painted row 2 (a 22px row would put it
        // in row 1).
        let row = 2.0;
        logic.handle(
            UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(area.x + 8.0 * 3.0 + 4.0, area.y + 16.0 * row + 8.0),
                modifiers: Modifiers::default(),
            },
            &mut backend,
        );

        let dispatched = &logic.sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| *event
                == UiEvent::Terminal(
                    WidgetId::new("term"),
                    quadraui::TerminalEvent::SelectStart { row: 2, col: 3 }
                )),
            "expected SelectStart at the painted row 2: {dispatched:?}"
        );
    }

    /// The binding-host leg of `CommandCenter`'s event parity: a
    /// `MouseDown` on each part of a `"command_center"` leaf resolves
    /// through [`InteractionEngine::handle`] to the same
    /// `UiEvent::CommandCenterActivated` the `TuiDriver` leg
    /// (`quadraui/tests/widget_contract_command_center.rs`) and the
    /// `a11y::route_widget_action` leg produce.
    #[test]
    fn click_on_a_command_center_part_dispatches_command_center_activated() {
        use quadraui::CommandCenterTarget;

        let id = WidgetId::new("titlebar:cc");
        let logic = CommandCenterLogic {
            cc: quadraui::CommandCenter {
                id,
                back_enabled: true,
                forward_enabled: true,
                search_label: "my-project".into(),
            },
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 5);

        for target in [
            CommandCenterTarget::Back,
            CommandCenterTarget::Forward,
            CommandCenterTarget::Search,
        ] {
            let (x, y) = quadraui::primitive::painted_sub_center(&id, target.sub_target())
                .expect("the command center records each part it painted");
            driver.dispatch(UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(x, y),
                modifiers: Modifiers::default(),
            });
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert!(
                dispatched
                    .iter()
                    .any(|(event, _)| *event
                        == UiEvent::CommandCenterActivated { widget: id, target }),
                "expected CommandCenterActivated {target:?}: {dispatched:?}"
            );
        }
    }

    /// Mirrors [`CommandCenterLogic`], painting one generic-widget leaf
    /// of any [`crate::extra_paint::WIDGET_KINDS`] kind.
    struct WidgetLeafLogic {
        kind: &'static str,
        widget: Rc<dyn DynWidget>,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl WidgetLeafLogic {
        fn new(kind: &'static str, widget: Rc<dyn DynWidget>) -> Self {
            Self {
                kind,
                widget,
                engine: InteractionEngine::new(),
                sink: RefCell::new(RecordingSink::default()),
            }
        }
    }

    impl AppLogic for WidgetLeafLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let extra = HashMap::new();
            let mut widgets: HashMap<String, Rc<dyn DynWidget>> = HashMap::new();
            widgets.insert(self.kind.to_string(), self.widget.clone());
            draw_bands(
                backend,
                RenderRefs {
                    text_display: None,
                    list: None,
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                    widgets: &widgets,
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
            );
            self.engine.render_overlays(backend);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// A left press and release at `(x, y)`, the gesture every desktop
    /// click is.
    fn click_through_the_engine(driver: &mut TuiDriver<WidgetLeafLogic>, x: f32, y: f32) {
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
            modifiers: Modifiers::default(),
        });
        driver.dispatch(UiEvent::MouseUp {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
        });
    }

    /// The binding-host leg of `StatusBar`'s event parity: a click on a
    /// clickable segment of a `"status_bar"` leaf resolves through
    /// [`InteractionEngine::handle`] to the same
    /// `UiEvent::StatusBar(bar, SegmentClicked { .. })` the `TuiDriver`
    /// leg (`quadraui/tests/widget_contract_status_bar.rs`) produces.
    /// `StatusBar` declares no a11y actions, so there is no
    /// `a11y::route_widget_action` leg.
    #[test]
    fn click_on_a_status_bar_segment_dispatches_segment_clicked() {
        let bar_id = WidgetId::new("status");
        let segment_id = WidgetId::new("status:branch");
        let segment = |text: &str, action_id: Option<WidgetId>| quadraui::StatusBarSegment {
            text: text.into(),
            fg: quadraui::Color::rgb(230, 230, 230),
            bg: quadraui::Color::rgb(40, 80, 120),
            bold: false,
            action_id,
        };
        let bar = quadraui::StatusBar {
            id: bar_id,
            left_segments: vec![segment("NORMAL", None), segment(" main ", Some(segment_id))],
            right_segments: vec![],
        };
        let mut driver = TuiDriver::new(WidgetLeafLogic::new("status_bar", Rc::new(bar)), 40, 5);

        let (x, y) =
            quadraui::primitive::painted_sub_center(&bar_id, quadraui::interactive::SubTarget(1))
                .expect("the status bar records each segment it painted");
        click_through_the_engine(&mut driver, x, y);

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| *event
                == UiEvent::StatusBar(
                    bar_id,
                    quadraui::StatusBarEvent::SegmentClicked { id: segment_id }
                )),
            "expected SegmentClicked for the clicked segment: {dispatched:?}"
        );
    }

    /// The binding-host leg of `ActivityBar`'s event parity: a click on
    /// the first row of an `"activity_bar"` leaf resolves through
    /// [`InteractionEngine::handle`] to the same
    /// `UiEvent::ActivityBar(bar, ItemClicked { .. })` the `TuiDriver` leg
    /// (`quadraui/tests/widget_contract_activity_bar.rs`) and the
    /// `a11y::route_widget_action` leg (`ActivityBar`'s own
    /// `widget_contract_tests`) produce.
    #[test]
    fn click_on_an_activity_bar_row_dispatches_item_clicked() {
        let bar_id = WidgetId::new("activity");
        let item = |id: &str, icon: &str| quadraui::ActivityItem {
            id: WidgetId::new(id),
            icon: icon.into(),
            tooltip: id.into(),
            is_active: false,
            is_keyboard_selected: false,
        };
        let bar = quadraui::ActivityBar {
            id: bar_id,
            top_items: vec![item("activity:explorer", "E"), item("activity:search", "S")],
            bottom_items: vec![],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
        };
        let mut driver = TuiDriver::new(WidgetLeafLogic::new("activity_bar", Rc::new(bar)), 40, 5);

        let (x, y) =
            quadraui::primitive::painted_sub_center(&bar_id, quadraui::interactive::SubTarget(0))
                .expect("the activity bar records each row it painted");
        click_through_the_engine(&mut driver, x, y);

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| *event
                == UiEvent::ActivityBar(
                    bar_id,
                    quadraui::ActivityBarEvent::ItemClicked {
                        id: WidgetId::new("activity:explorer")
                    }
                )),
            "expected ItemClicked for the first row: {dispatched:?}"
        );
    }

    /// Enter on a focused layout-tree `Button` round-trips through
    /// [`InteractionEngine::handle`] as the same `UiEvent::ButtonActivated`
    /// a pointer click produces — the keyboard-activation half of this
    /// engine's `Button` coverage, exercised via
    /// [`InteractionEngine::dispatch_dyn_widget_key`] rather than
    /// [`InteractionEngine::handle`]'s `MouseDown`/`DoubleClick` routing.
    /// Unlike the pointer click, this fully replaces the raw `KeyPressed`
    /// — the same generic `"button"`-kind [`HitDetail::WidgetBody`]
    /// replacement [`InteractionEngine::dispatch_dyn_widget_key`] produces
    /// for any focused [`crate::primitive::DynWidget`] leaf.
    #[test]
    fn enter_on_a_focused_button_dispatches_button_activated() {
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
                UiEvent::ButtonActivated {
                    widget: WidgetId::new("go")
                },
                Some(HitDetail::WidgetBody)
            )],
            "Enter on a focused Button must dispatch exactly the generic \
             ButtonActivated, replacing the raw KeyPressed: {:?}",
            dispatched
        );
    }

    /// Space does the same as Enter (see
    /// [`enter_on_a_focused_button_dispatches_button_activated`]).
    #[test]
    fn space_on_a_focused_button_dispatches_button_activated() {
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
                UiEvent::ButtonActivated {
                    widget: WidgetId::new("go")
                },
                Some(HitDetail::WidgetBody)
            )],
            "Space on a focused Button must dispatch exactly the generic \
             ButtonActivated, replacing the raw KeyPressed: {:?}",
            dispatched
        );
    }

    /// A `MouseMoved` landing on a painted button, then one moving off it,
    /// round-trips through [`InteractionEngine::handle`] as a hit-tested
    /// `UiEvent::MouseEntered` followed later by a `UiEvent::MouseLeft`,
    /// each carrying the button's own generic `HitDetail::WidgetBody` —
    /// the real hover signal a binding's own toolbar/tree/details-pane
    /// hover tooltip needs, synthesized the same way for every binding
    /// since it lives in this shared engine rather than each binding
    /// re-deriving hit-testing from the raw position itself.
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
                    crate::hit::PaintedHit::Widget { id, area, widget }
                        if *id == WidgetId::new("go") && widget.as_any().is::<Button>() =>
                    {
                        Some(*area)
                    }
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
                    Some(HitDetail::WidgetBody)
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
                Some(HitDetail::WidgetBody)
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
    /// [`FormDialog`] hosting a `text_input` field emits the
    /// normal `UiEvent::Form` event for typing, and Tab never escapes
    /// the modal to the main tree's own button underneath it —
    /// exercised through the real `TuiDriver::dispatch` path (not a
    /// direct `engine.handle` call), so `quadraui::runtime::preprocess_event`'s
    /// own global Tab-cycle is actually in the loop, the same way a real
    /// binding's runner would drive it. See [`InteractionEngine::tab_stops`]'s
    /// own doc for why trapping Tab needs this method's cooperation, not
    /// just [`FormDialog`]'s internal handling. A `switch`
    /// (`FieldKind::Toggle`) field's own normal-event path is covered by
    /// the popover test below instead — computing a click position for
    /// it needs `FormController::field_rect`, a method this test's own
    /// `FormDialog` has no equivalent public accessor for.
    #[test]
    fn open_form_dialog_with_text_input_and_toggle_traps_tab_and_emits_normal_form_events() {
        use quadraui::compose::FormDialog;
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

        let mut dialog = FormDialog::new(WidgetId::new("prefs"), "Preferences").with_buttons(vec![
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
        dialog.form = Some(form);

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
                .and_then(|d| d.form.as_ref())
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
                    PaintedHit::Widget { id, area, .. } if *id == WidgetId::new("go") => {
                        Some(*area)
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
        use quadraui::{
            Anchor, FieldKind, Float, FloatMeasure, Form, FormEvent, FormField, Key, NamedKey, Side,
        };

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
                    descriptor: LeafDescriptor::Widget {
                        kind: "button",
                        widget: Rc::new(Button::new(WidgetId::new("submit"), "Submit")),
                    },
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
                    PaintedHit::Widget { id, area, .. } if *id == WidgetId::new("go") => {
                        Some(*area)
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
        // main tree) and emits the generic `UiEvent::ButtonActivated` shape.
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
                    UiEvent::ButtonActivated { widget } if *widget == WidgetId::new("submit")
                )),
                "clicking the button should emit the generic ButtonActivated event: {:?}",
                dispatched
            );
        }
        driver.app().sink.borrow_mut().dispatched.clear();

        // The click above left "submit" focused; Enter on it must activate
        // it too, the same as a main-tree button already does through
        // `InteractionEngine::dispatch_dyn_widget_key`.
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
                    UiEvent::ButtonActivated { widget } if *widget == WidgetId::new("submit")
                )),
                "Enter on the focused popover button should emit the generic \
                 ButtonActivated event: {:?}",
                dispatched
            );
        }

        driver.app().sink.borrow_mut().dispatched.clear();

        // A double-click on the same button must activate it only once:
        // `Button::handle` only matches `PointerKind::Down` on the main
        // tree, ignoring `PointerKind::DoubleClick` outright, so this
        // subtree's own click arm must not fire `ButtonActivated` for a
        // `DoubleClick` either.
        driver.dispatch(UiEvent::DoubleClick {
            widget: None,
            position: Point::new(
                submit_rect.x + submit_rect.width * 0.5,
                submit_rect.y + submit_rect.height * 0.5,
            ),
        });
        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert!(
                !dispatched
                    .iter()
                    .any(|(ev, _)| matches!(ev, UiEvent::ButtonActivated { .. })),
                "a double-click on the popover button must not emit a \
                 second ButtonActivated: {:?}",
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
            striped: false,
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
                    PaintedHit::Widget { id, area, .. } if *id == WidgetId::new("go") => {
                        Some(*area)
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
                    descriptor: LeafDescriptor::Widget {
                        kind: "button",
                        widget: Rc::new(Button::new(WidgetId::new("reset"), "Reset")),
                    },
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
            descriptor: LeafDescriptor::Widget {
                kind: "button",
                widget: Rc::new(Button::new(WidgetId::new("apply"), "Apply")),
            },
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
                backend.focus_tree().focused_id().copied(),
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
            backend.focus_tree().focused_id().copied(),
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
                font_role: Default::default(),
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

    fn overlay_driver() -> TuiDriver<MinimalLogic> {
        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        TuiDriver::new(logic, 40, 20)
    }

    fn overlay_events(driver: &TuiDriver<MinimalLogic>) -> Vec<UiEvent> {
        driver
            .app()
            .sink
            .borrow()
            .dispatched
            .iter()
            .map(|(event, _)| event.clone())
            .collect()
    }

    fn two_completions() -> quadraui::Completions {
        serde_json::from_value(serde_json::json!({
            "id": "completions",
            "selected_idx": 0,
            "items": [
                {"label": {"spans": [{"text": "map", "fg": null, "bg": null}]}},
                {"label": {"spans": [{"text": "filter", "fg": null, "bg": null}]}},
            ],
        }))
        .expect("completions decode")
    }

    /// A press on a completion row goes through `Completions`' own
    /// `Interactive::handle`: it moves the highlight (written back onto
    /// the overlay the host owns) without taking focus from whatever the
    /// popup floats over, and only a double-click accepts.
    #[test]
    fn a_completions_overlay_routes_presses_through_its_own_handle() {
        let mut driver = overlay_driver();
        let (_backend, app) = driver.parts_mut();
        app.engine
            .show_completions(two_completions(), Point::new(2.0, 2.0));
        driver.render();

        let (x, y) = driver.find("filter").expect("the second item paints");
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
            modifiers: Modifiers::default(),
        });
        let id = WidgetId::new("completions");
        assert_eq!(
            overlay_events(&driver),
            vec![UiEvent::List(
                id,
                quadraui::ListViewEvent::SelectionChanged { idx: 1 }
            )]
        );
        assert_eq!(
            driver
                .app()
                .engine
                .find_completions()
                .map(|(c, _)| c.selected_idx),
            Some(1)
        );

        driver.dispatch(UiEvent::DoubleClick {
            widget: None,
            position: Point::new(x, y),
        });
        assert_eq!(
            overlay_events(&driver).last(),
            Some(&UiEvent::CompletionItemActivated(id, 1))
        );
        assert!(driver.app().engine.find_completions().is_none());
    }

    /// A wheel notch over an overflowing rich-text popup scrolls the popup
    /// through its own `Interactive::handle` (reported as
    /// `ScrollOffsetChanged` and written back onto the overlay) instead of
    /// falling through to whatever is painted behind it.
    #[test]
    fn a_wheel_over_a_rich_text_popup_scrolls_the_popup() {
        let lines: Vec<quadraui::StyledText> = (0..12)
            .map(|i| {
                serde_json::from_value(serde_json::json!({
                    "spans": [{"text": format!("line {i}"), "fg": null, "bg": null}]
                }))
                .expect("line decodes")
            })
            .collect();
        let popup = quadraui::RichTextPopup {
            id: WidgetId::new("popup"),
            line_text: (0..12).map(|i| format!("line {i}")).collect(),
            lines,
            line_scales: vec![],
            scroll_top: 0,
            max_visible_rows: 3,
            has_focus: false,
            selection: None,
            links: vec![],
            focused_link: None,
            placement: Default::default(),
            padding: 1.0,
            fg: None,
            bg: None,
            font_role: Default::default(),
        };
        let mut driver = overlay_driver();
        let (_backend, app) = driver.parts_mut();
        app.engine.show_rich_text_popup(popup, Point::new(2.0, 2.0));
        driver.render();

        let (x, y) = driver.find("line 1").expect("the popup body paints");
        driver.dispatch(UiEvent::Scroll {
            widget: None,
            delta: quadraui::ScrollDelta { x: 0.0, y: -1.0 },
            position: Point::new(x, y),
        });
        let scrolled: Vec<UiEvent> = overlay_events(&driver)
            .into_iter()
            .filter(|e| matches!(e, UiEvent::ScrollOffsetChanged { .. }))
            .collect();
        assert_eq!(scrolled.len(), 1, "{:?}", overlay_events(&driver));
        let UiEvent::ScrollOffsetChanged { widget, new_offset } = scrolled[0] else {
            unreachable!()
        };
        assert_eq!(widget, WidgetId::new("popup"));
        assert!(new_offset > 0);
        assert_eq!(
            driver
                .app()
                .engine
                .find_rich_text_popup()
                .map(|(p, _)| p.scroll_top),
            Some(new_offset)
        );
    }

    fn short_rich_text_popup() -> quadraui::RichTextPopup {
        quadraui::RichTextPopup {
            id: WidgetId::new("popup"),
            line_text: vec!["short note".to_string()],
            lines: vec![serde_json::from_value(serde_json::json!({
                "spans": [{"text": "short note", "fg": null, "bg": null}]
            }))
            .expect("line decodes")],
            line_scales: vec![],
            scroll_top: 0,
            max_visible_rows: 3,
            has_focus: false,
            selection: None,
            links: vec![],
            focused_link: None,
            placement: Default::default(),
            padding: 1.0,
            fg: None,
            bg: None,
            font_role: Default::default(),
        }
    }

    /// A double-click or a wheel notch on a rich-text popup's body — which
    /// the popup itself ignores — is still swallowed by the popup, never
    /// reaching the app or whatever is painted beneath it.
    #[test]
    fn input_the_rich_text_popup_ignores_still_never_clicks_through_it() {
        let mut driver = overlay_driver();
        let (_backend, app) = driver.parts_mut();
        app.engine
            .show_rich_text_popup(short_rich_text_popup(), Point::new(2.0, 2.0));
        driver.render();
        let (x, y) = driver.find("short note").expect("the popup body paints");

        driver.dispatch(UiEvent::DoubleClick {
            widget: None,
            position: Point::new(x, y),
        });
        driver.dispatch(UiEvent::Scroll {
            widget: None,
            delta: quadraui::ScrollDelta { x: 0.0, y: -1.0 },
            position: Point::new(x, y),
        });

        assert_eq!(overlay_events(&driver), Vec::<UiEvent>::new());
        assert!(
            driver.app().engine.find_rich_text_popup().is_some(),
            "a press on the popup itself keeps it open"
        );
    }

    /// The same for a completions list: a wheel notch over a list too
    /// short to scroll is swallowed rather than scrolling the content
    /// behind it.
    #[test]
    fn a_wheel_notch_on_a_completions_list_never_scrolls_what_is_beneath_it() {
        let mut driver = overlay_driver();
        let (_backend, app) = driver.parts_mut();
        app.engine
            .show_completions(two_completions(), Point::new(2.0, 2.0));
        driver.render();
        let (x, y) = driver.find("filter").expect("the second item paints");

        driver.dispatch(UiEvent::Scroll {
            widget: None,
            delta: quadraui::ScrollDelta { x: 0.0, y: -1.0 },
            position: Point::new(x, y),
        });

        assert!(
            !overlay_events(&driver)
                .iter()
                .any(|e| matches!(e, UiEvent::Scroll { .. })),
            "{:?}",
            overlay_events(&driver)
        );
        assert!(driver.app().engine.find_completions().is_some());
    }

    /// A generic widget leaf's interaction state and the host-resolved
    /// container values live in the one store the engine exposes, so a
    /// third binding reads both through [`InteractionEngine::widget_state`].
    #[test]
    fn generic_widget_state_lives_in_the_engines_one_store() {
        let mut driver = overlay_driver();
        driver.render();
        let (x, y) = driver.find("Go").expect("the button paints");
        driver.click(x, y);
        let store = driver.app().engine.widget_state().borrow();
        assert!(
            store.contains(&WidgetId::new("go")),
            "the clicked button's own state is stored alongside the host's values"
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
                font_role: Default::default(),
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
                font_role: Default::default(),
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
                    text_display: None,
                    list: None,
                    tree: None,
                    data_table: Some(&self.data_table),
                    tab_bar: None,
                    extra: &extra,
                    widgets: crate::render::no_widgets(),
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
            );
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
        bordered_data_table_driver(selected_idx, false)
    }

    /// Same table as [`data_table_driver`], with `bordered` threaded through —
    /// used by the drag tests that must confirm a resize tracks the pointer
    /// identically whether or not the focus-ring inset is reserved.
    fn bordered_data_table_driver(
        selected_idx: Option<usize>,
        bordered: bool,
    ) -> TuiDriver<DataTableLogic> {
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
            bordered,
            striped: false,
            glyphs: None,
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

    /// A column-resize drag on a `bordered` table must grow the column by
    /// exactly the pointer's own movement — same as the non-bordered case
    /// above — regression for `DataTableColumnDrag::width_at` re-deriving
    /// `DataTableLayout::content_x` without undoing `inset_x` first, which
    /// resolved the dragged width one cell too wide whenever `bordered` was
    /// set.
    #[test]
    fn dragging_a_header_divider_on_a_bordered_table_grows_by_the_pointer_delta() {
        use quadraui::DataTableEvent;

        let mut driver = bordered_data_table_driver(None, true);
        let (area, layout) = data_table_geometry(&driver);
        let original_width = layout.columns[0].width;
        let divider_y = area.y + layout.inset_y + layout.header_height / 2.0;
        let divider_point = Point::new(
            area.x + layout.inset_x + layout.columns[0].x + original_width,
            divider_y,
        );

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: divider_point,
            modifiers: Modifiers::default(),
        });
        let delta = 5.0;
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(divider_point.x + delta, divider_y),
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
            (width - (original_width + delta)).abs() < 0.01,
            "a bordered table's resize must track the pointer 1:1, not add an \
             extra cell for the inset: original={original_width}, delta={delta}, new={width}"
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
                    text_display: None,
                    list: None,
                    tree: Some(&resolved),
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                    widgets: crate::render::no_widgets(),
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
            );
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
                    text_display: None,
                    list: Some(&resolved),
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                    widgets: crate::render::no_widgets(),
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
            );
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
            striped: false,
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
            let viewport_rows =
                quadraui::DrawsPrimitives::layout_primitive(backend, table_rect, &self.table)
                    .layout
                    .visible_rows;
            let resolved =
                resolve_data_table_cursor(self.engine.widget_state(), &self.table, viewport_rows);

            let mut hits = Vec::new();
            let extra = HashMap::new();
            draw_bands(
                backend,
                RenderRefs {
                    text_display: None,
                    list: None,
                    tree: None,
                    data_table: Some(&resolved),
                    tab_bar: None,
                    extra: &extra,
                    widgets: crate::render::no_widgets(),
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
            );
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
            striped: false,
            glyphs: None,
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
                    text_display: None,
                    list: Some(&self.list),
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                    widgets: crate::render::no_widgets(),
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
            );
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
            striped: false,
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

    /// Dragging a `List` row onto another row of the same list reorders it
    /// -- the generic draggable/drop-target gesture
    /// ([`crate::drag::GestureDrag`]) over a leaf `InteractionEngine::
    /// drag_roles` marks both `draggable` and `drop_target`: `MouseDown` on
    /// row 0 arms the gesture and dispatches `DragStarted`, `MouseMoved`
    /// onto row 2 dispatches `DragOver` once the target changes, and
    /// `MouseUp` still over row 2 dispatches `Dropped` carrying both rows'
    /// own `HitDetail::ListItem` -- the shape a host actually moving the
    /// dragged row from index 0 to index 2 in its own backing collection
    /// reads back. Also checks that the ordinary pointer-click selection
    /// semantics the same `MouseDown` already produces (`ListViewEvent::
    /// SelectionChanged`) are untouched: arming a gesture is additive, not
    /// a replacement, for a leaf with its own click semantics.
    #[test]
    fn dragging_a_list_row_onto_another_row_reports_dropped_with_both_row_indices() {
        use crate::wire::DragRole;
        use quadraui::ListViewEvent;

        let mut driver = list_driver(3, 0);
        driver.render();
        driver.app().engine.set_drag_roles(HashMap::from([(
            WidgetId::new("list"),
            DragRole {
                draggable: true,
                drop_target: true,
            },
        )]));

        let list_rect = driver
            .app()
            .engine
            .tab_stops()
            .iter()
            .find_map(|(id, rect)| (*id == WidgetId::new("list")).then_some(*rect))
            .expect("the list leaf should be a tab stop");
        let line_height = {
            let (backend, _) = driver.parts_mut();
            backend.measure().line_height
        };
        let row_point = |row: usize| {
            Point::new(
                list_rect.x + 1.0,
                list_rect.y + line_height * (row as f32 + 0.5),
            )
        };

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: row_point(0),
            modifiers: Modifiers::default(),
        });
        driver.dispatch(UiEvent::MouseMoved {
            position: row_point(2),
            buttons: quadraui::ButtonMask {
                left: true,
                right: false,
                middle: false,
            },
        });
        driver.dispatch(UiEvent::MouseUp {
            widget: None,
            button: MouseButton::Left,
            position: row_point(2),
        });

        let app = driver.app();
        let sink = app.sink.borrow();
        assert!(
            sink.dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::List(id, ListViewEvent::SelectionChanged { idx: 0 })
                    if *id == WidgetId::new("list")
            )),
            "picking up row 0 must still select it, same as a plain click: {:?}",
            sink.dispatched
        );
        assert!(
            matches!(
                sink.drag_gesture_dispatched.first(),
                Some(DragGestureWireEvent::DragStarted {
                    source,
                    source_detail: HitDetail::ListItem { index: 0 },
                }) if *source == WidgetId::new("list")
            ),
            "expected DragStarted at row 0: {:?}",
            sink.drag_gesture_dispatched
        );
        assert!(
            sink.drag_gesture_dispatched.iter().any(|event| matches!(
                event,
                DragGestureWireEvent::DragOver {
                    source,
                    source_detail: HitDetail::ListItem { index: 0 },
                    target,
                    target_detail: HitDetail::ListItem { index: 2 },
                } if *source == WidgetId::new("list") && *target == WidgetId::new("list")
            )),
            "expected a DragOver onto row 2: {:?}",
            sink.drag_gesture_dispatched
        );
        assert!(
            matches!(
                sink.drag_gesture_dispatched.last(),
                Some(DragGestureWireEvent::Dropped {
                    source,
                    source_detail: HitDetail::ListItem { index: 0 },
                    target,
                    target_detail: HitDetail::ListItem { index: 2 },
                }) if *source == WidgetId::new("list") && *target == WidgetId::new("list")
            ),
            "expected Dropped from row 0 onto row 2: {:?}",
            sink.drag_gesture_dispatched
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
                    text_display: Some(&self.text_display),
                    list: None,
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                    widgets: crate::render::no_widgets(),
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
            );
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
                        descriptor: LeafDescriptor::Widget {
                            kind: "button",
                            widget: Rc::new(Button::new(WidgetId::new("left-btn"), "L")),
                        },
                        scroll_px: None,
                    }),
                    LayoutNode::Leaf(LayoutLeaf {
                        id: "right".to_string(),
                        descriptor: LeafDescriptor::Widget {
                            kind: "button",
                            widget: Rc::new(Button::new(WidgetId::new("right-btn"), "R")),
                        },
                        scroll_px: None,
                    }),
                ],
            });
            paint_layout(backend, area, &node, &mut hits);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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

        // Ratio 0.6 over the 39 cells a 40-wide viewport shares out puts
        // the unrounded divider at 23.4, which the cell layout places on
        // the whole cell 23.
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
        assert_eq!(
            divider.position, 23.0,
            "the cell layout places the divider on the cell it paints at"
        );
        // Both leaves end at the divider cell, so a press on the cell the
        // divider paints at — a realistic integer mouse column — lands on
        // the divider, not on a leaf.
        let divider_cell = divider.position;

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
        // The exact inverse of the layout: the cursor's offset over the
        // span the divider shares out, (23 + 10 - 0) / (40 - 1).
        let expected_ratio =
            (divider_cell + 10.0 - divider.axis_start) / (divider.axis_size - divider.thickness);
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
                    descriptor: LeafDescriptor::Widget {
                        kind: "button",
                        widget: Rc::new(Button::new(WidgetId::new("left-btn"), "L")),
                    },
                    scroll_px: None,
                })),
                second: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "right".to_string(),
                    descriptor: LeafDescriptor::Widget {
                        kind: "button",
                        widget: Rc::new(Button::new(WidgetId::new("right-btn"), "R")),
                    },
                    scroll_px: None,
                })),
                spacing: Spacing::default(),
            });
            paint_layout(backend, area, &node, &mut hits);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
            ..Default::default()
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
                ..Default::default()
            };
            let entries = (0..self.tab_count)
                .map(|i| LayoutTabEntry {
                    id: format!("content-{i}"),
                    content: Box::new(LayoutNode::Leaf(LayoutLeaf {
                        id: format!("content-{i}-btn"),
                        descriptor: LeafDescriptor::Widget {
                            kind: "button",
                            widget: Rc::new(Button::new(
                                WidgetId::new(format!("content-{i}-btn")),
                                format!("Tab {i} body"),
                            )),
                        },
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
            paint_layout(backend, area, &node, &mut hits);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
                crate::hit::PaintedHit::Widget { id, .. } if *id == WidgetId::new("content-0-btn")
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
                crate::hit::PaintedHit::Widget { id, .. } if *id == WidgetId::new("content-1-btn")
            )),
            "the content pane must follow the clicked tab, not stay on the descriptor's own tab 0"
        );
        assert!(
            !hits.iter().any(|hit| matches!(
                hit,
                crate::hit::PaintedHit::Widget { id, .. } if *id == WidgetId::new("content-0-btn")
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
                    descriptor: LeafDescriptor::Widget {
                        kind: "button",
                        widget: Rc::new(Button::new(WidgetId::new("body-btn"), "Body")),
                    },
                    scroll_px: None,
                })),
            });
            paint_layout(backend, area, &node, &mut hits);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
    /// arm, which records the panel's header as a
    /// [`crate::hit::PaintedHit::Widget`] so the click routes through
    /// `SidebarPanel`'s own `Interactive` impl instead of staying a raw
    /// `MouseDown`. This is the binding-host leg of the three-way event
    /// parity whose `TuiDriver` and `a11y::route_widget_action` legs live
    /// in `quadraui`'s `tests/widget_contract_sidebar_panel.rs` and
    /// `primitives/sidebar_panel.rs`: all three resolve to the same event.
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

    /// Mirrors [`FormLogic`], but paints one standalone `"toolbar"` widget
    /// slot, distinct from the `SidebarPanel`-embedded shape the test above
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
            let mut widgets: HashMap<String, std::rc::Rc<dyn DynWidget>> = HashMap::new();
            widgets.insert(
                "toolbar".to_string(),
                std::rc::Rc::new(self.toolbar.clone()) as std::rc::Rc<dyn DynWidget>,
            );
            draw_bands(
                backend,
                RenderRefs {
                    text_display: None,
                    list: None,
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: crate::render::no_extras(),
                    widgets: &widgets,
                    list_scroll_px: None,
                    tree_scroll_px: None,
                    data_table_scroll_px: None,
                },
                &mut hits,
            );
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
        // The click parks the bar's keyboard cursor on the button, so the
        // bar takes focus first.
        assert_eq!(
            dispatched.len(),
            2,
            "expected FocusChanged then the Toolbar click: {:?}",
            dispatched
        );
        assert!(
            matches!(&dispatched[0].0, UiEvent::FocusChanged(Some(id)) if *id == WidgetId::new("toolbar:main")),
            "{dispatched:?}"
        );
        assert!(
            matches!(
                &dispatched[1].0,
                UiEvent::Toolbar(
                    bar,
                    ToolbarEvent::ButtonClicked { id, is_active: true },
                ) if *bar == WidgetId::new("toolbar:main") && *id == WidgetId::new("filter")
            ),
            "expected Toolbar(toolbar:main, ButtonClicked{{id: filter, is_active: true}}): {:?}",
            dispatched
        );
    }

    /// The binding-host leg of the menu widgets' three-way event parity:
    /// a click on the first enabled button of a standalone `"toolbar"`
    /// leaf dispatches exactly the toolbar event the widget contract's own
    /// TuiDriver leg (`quadraui/tests/widget_contract_menus_event_parity.rs`)
    /// and its `a11y::route_widget_action` leg resolve the same bar to.
    #[test]
    fn toolbar_click_matches_the_widget_contract_parity_event() {
        use quadraui::{Toolbar, ToolbarButton};

        let action = |id: &str, label: &str, enabled: bool| ToolbarButton::Action {
            spec: None,
            id: WidgetId::new(id),
            label: label.to_string(),
            icon: None,
            key_hint: None,
            enabled,
            is_active: false,
            tooltip: String::new(),
        };
        let logic = ToolbarLogic {
            toolbar: Toolbar::new(WidgetId::new("tb")).with_buttons(vec![
                action("tb:a", "A", true),
                ToolbarButton::Separator,
                action("tb:b", "B", false),
            ]),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 5);
        driver.render();
        let (x, y) = driver.find("A").expect("the first button paints");
        driver.click(x, y);

        let dispatched: Vec<UiEvent> = driver
            .app()
            .sink
            .borrow()
            .dispatched
            .iter()
            .map(|(event, _)| event.clone())
            .filter(|event| matches!(event, UiEvent::Toolbar(..)))
            .collect();
        assert_eq!(
            dispatched,
            vec![UiEvent::Toolbar(
                WidgetId::new("tb"),
                ToolbarEvent::ButtonClicked {
                    id: WidgetId::new("tb:a"),
                    is_active: true,
                },
            )]
        );
    }

    /// The tooltip overlay is sized by `Tooltip`'s own measure: on a cell
    /// grid, the widest row plus border and padding columns, and the rows
    /// plus a top and bottom border.
    #[test]
    fn a_tooltip_overlay_is_sized_by_the_tooltips_own_measure() {
        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        let (backend, app) = driver.parts_mut();
        let tooltip = quadraui::Tooltip::new(WidgetId::new("tip"), "abc");
        let layout = app
            .engine
            .tooltip_layout(backend, &tooltip, Rect::new(5.0, 5.0, 1.0, 1.0));
        assert_eq!((layout.bounds.width, layout.bounds.height), (6.0, 3.0));
    }

    /// The binding-host leg of the context menu's event parity: a click
    /// on a popup row opened through [`InteractionEngine::open_context_menu`]
    /// dispatches the `UiEvent::ContextMenuItemActivated` the widget
    /// contract's TuiDriver and `a11y::route_widget_action` legs resolve
    /// the same row to.
    #[test]
    fn context_menu_click_matches_the_widget_contract_parity_event() {
        use quadraui::ContextMenuItem;

        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        {
            let (backend, app) = driver.parts_mut();
            let item = |id: &str, label: &str| ContextMenuItem {
                id: Some(WidgetId::new(id)),
                label: quadraui::types::StyledText::plain(label),
                ..Default::default()
            };
            app.engine.open_context_menu(
                vec![item("copy", "Copy"), item("paste", "Paste")],
                Point::new(1.0, 1.0),
                backend,
            );
        }
        driver.render();
        let (x, y) = driver.find("Copy").expect("the popup paints its rows");
        driver.click(x, y);

        let dispatched: Vec<UiEvent> = driver
            .app()
            .sink
            .borrow()
            .dispatched
            .iter()
            .map(|(event, _)| event.clone())
            .collect();
        assert_eq!(
            dispatched,
            vec![UiEvent::ContextMenuItemActivated(WidgetId::new("copy"))]
        );
    }

    /// Tab must land focus on a standalone toolbar (a focus stop whenever
    /// it has at least one enabled `Action`), the arrow keys move the
    /// bar's own keyboard cursor, and Enter activates the button under it
    /// — the same `UiEvent::Toolbar(ButtonClicked)` shape a mouse click
    /// produces.
    #[test]
    fn tab_focuses_a_toolbar_and_enter_activates_its_focused_button() {
        let logic = ToolbarLogic {
            toolbar: two_action_toolbar(),
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

        // The first arrow press parks the cursor on the first button, the
        // second moves it to "Reset".
        for key in [NamedKey::Right, NamedKey::Right, NamedKey::Enter] {
            driver.dispatch(UiEvent::KeyPressed {
                key: Key::Named(key),
                modifiers: Modifiers::default(),
                repeat: false,
            });
        }

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
                        descriptor: LeafDescriptor::Widget {
                            kind: "button",
                            widget: Rc::new(Button::new(WidgetId::new(label.clone()), label)),
                        },
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
                        descriptor: LeafDescriptor::Widget {
                            kind: "button",
                            widget: Rc::new(Button::new(WidgetId::new(label.clone()), label)),
                        },
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
            paint_layout(backend, area, &node, &mut hits);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
                descriptor: LeafDescriptor::Widget {
                    kind: "button",
                    widget: Rc::new(Button::new(WidgetId::new(label.clone()), label)),
                },
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
            paint_layout(backend, area, &node, &mut hits);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
                PaintedHit::Widget { id, widget, .. } if widget.as_any().is::<Button>() => {
                    Some(id.as_str().to_string())
                }
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
        let clicked = driver
            .app()
            .sink
            .borrow()
            .dispatched
            .iter()
            .find_map(|(event, _detail)| match event {
                UiEvent::ButtonActivated { widget } => Some(widget.as_str().to_string()),
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
    /// entirely by `backend.focus_tree().focused_id()`
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
            paint_layout(backend, area, &node, &mut hits);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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

    // ─── Scripted widgets on the generic widget routing ───
    //
    // A `ScriptWidget` built directly in Rust test code (mirroring
    // `crate::scripted`'s own `Swatch` test widget — no real binding
    // callback object anywhere in scope), placed in a generic widget leaf
    // and driven through a real `InteractionEngine::handle`, proving a
    // region click and a focused key press both reach the script's own
    // `on_event` through `DynWidget::handle`, with no `W`-specific code in
    // `hit.rs`/`semantic.rs`/`engine.rs`.

    /// Records every call [`ScriptWidget::on_event`] receives into `log`,
    /// so a test can assert exactly what reached it. One region (its own
    /// right half, same shape `crate::scripted`'s own `Swatch` test
    /// widget uses) — everything else is the minimum `ScriptWidget` needs
    /// to paint/measure at all.
    struct RecordingScriptWidget {
        log: Rc<RefCell<Vec<&'static str>>>,
    }

    impl crate::scripted::ScriptWidget for RecordingScriptWidget {
        fn measure(
            &self,
            _ctx: &quadraui::primitive::LayoutCtx,
            constraints: crate::scripted::ScriptedConstraints,
        ) -> crate::scripted::ScriptedSize {
            crate::scripted::ScriptedSize {
                width: constraints.max_width.unwrap_or(10.0),
                height: constraints.max_height.unwrap_or(4.0),
            }
        }

        fn paint(
            &self,
            _ctx: &quadraui::primitive::LayoutCtx,
            _props: &crate::scripted::ScriptedProps,
            rect: Rect,
        ) -> crate::scripted::ScriptedPaint {
            crate::scripted::ScriptedPaint {
                ops: Vec::new(),
                regions: vec![quadraui::CanvasRegion {
                    id: WidgetId::new("widget:region"),
                    rect: Rect::new(rect.width / 2.0, 0.0, rect.width / 2.0, rect.height),
                    cursor: None,
                }],
            }
        }

        fn on_event(
            &self,
            event: &crate::scripted::ScriptedInputEvent,
            _props: &crate::scripted::ScriptedProps,
            _state: &crate::scripted::ScriptedState,
        ) -> bool {
            match event {
                crate::scripted::ScriptedInputEvent::Canvas(
                    quadraui::CanvasEvent::RegionClicked { .. },
                ) => {
                    self.log.borrow_mut().push("clicked");
                    true
                }
                crate::scripted::ScriptedInputEvent::Key(_) => {
                    self.log.borrow_mut().push("key");
                    true
                }
                _ => false,
            }
        }
    }

    /// An [`AppLogic`] hosting exactly one `"widget"` leaf wrapping a
    /// [`crate::scripted::ScriptedPrimitive<RecordingScriptWidget>`] —
    /// built directly rather than decoded, the same way a binding's own
    /// render pass splices one in.
    struct ScriptedWidgetLogic {
        primitive: crate::scripted::ScriptedPrimitive<RecordingScriptWidget>,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for ScriptedWidgetLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let node = LayoutNode::Leaf(LayoutLeaf {
                id: "widget".to_string(),
                descriptor: LeafDescriptor::Widget {
                    kind: crate::scripted::SCRIPTED_WIDGET_KIND,
                    widget: Rc::new(self.primitive.clone()),
                },
                scroll_px: None,
            });
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// A `MouseDown` landing on the scripted widget's declared region
    /// reaches the script's own `on_event` through the generic widget
    /// routing, and the app receives the same `UiEvent::Canvas` region
    /// click a `Canvas` leaf reports. A focused `KeyPressed` afterward
    /// reaches the script too, and — since the script lets keys travel on —
    /// the app still receives the key press, with a repaint requested.
    #[test]
    fn scripted_widget_region_click_and_focused_key_both_forward_to_its_own_on_event() {
        let log: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
        let primitive = crate::scripted::ScriptedPrimitive::new(
            WidgetId::new("widget"),
            RecordingScriptWidget { log: log.clone() },
            true,
        );
        let logic = ScriptedWidgetLogic {
            primitive,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 10, 4);
        driver.render();

        // `RecordingScriptWidget::paint`'s region covers the right half
        // of a 10x4 TUI viewport: x in [5, 10), y in [0, 4).
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            position: Point::new(8.0, 2.0),
        });
        assert_eq!(
            log.borrow().as_slice(),
            ["clicked"],
            "a region click must reach the script's own on_event exactly once"
        );
        assert!(
            driver
                .app()
                .sink
                .borrow()
                .dispatched
                .iter()
                .any(|(event, _)| *event
                    == UiEvent::Canvas(
                        WidgetId::new("widget"),
                        quadraui::CanvasEvent::RegionClicked {
                            region: WidgetId::new("widget:region"),
                            button: MouseButton::Left,
                            modifiers: Modifiers::default(),
                            x: 8.0,
                            y: 2.0,
                        },
                    )),
            "{:?}",
            driver.app().sink.borrow().dispatched
        );

        // The `MouseDown` above already click-to-focused the widget (its
        // `FocusSpec::Stop`, since it was built `focusable: true`) — so
        // this `KeyPressed` reaches it with no further focus bookkeeping
        // needed.
        {
            let (backend, _app) = driver.parts_mut();
            assert_eq!(
                backend.focus_tree().focused_id().copied(),
                Some(WidgetId::new("widget")),
                "the click above must have already focused the widget"
            );
        }
        let key = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let reaction = driver.dispatch(key.clone());
        assert_eq!(
            log.borrow().as_slice(),
            ["clicked", "key"],
            "a key press while focused must also reach the script's own on_event"
        );
        assert_eq!(
            driver
                .app()
                .sink
                .borrow()
                .dispatched
                .last()
                .map(|(event, _)| event),
            Some(&key)
        );
        assert_eq!(reaction, Reaction::Redraw);
    }

    /// A `MouseDown` landing OUTSIDE the scripted widget's declared
    /// region (but still inside its own body) never reaches the script's
    /// `on_event`, mirroring a real `Canvas` leaf's own "outside every
    /// declared region" posture.
    #[test]
    fn a_click_outside_every_declared_region_never_reaches_on_event() {
        let log: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
        let primitive = crate::scripted::ScriptedPrimitive::new(
            WidgetId::new("widget"),
            RecordingScriptWidget { log: log.clone() },
            true,
        );
        let logic = ScriptedWidgetLogic {
            primitive,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 10, 4);
        driver.render();

        // The left half (x in [0, 5)) is inside the widget's own body but
        // outside its one declared region.
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            position: Point::new(1.0, 2.0),
        });
        assert!(
            log.borrow().is_empty(),
            "a click outside every declared region must never reach on_event: {:?}",
            log.borrow()
        );
    }

    #[test]
    fn theme_token_looks_up_a_colour_field_by_name() {
        let theme = quadraui::Theme::default();
        let looked_up = theme_token(&theme, "accent_fg").expect("accent_fg is a real field");
        let expected = serde_json::to_value(theme.accent_fg).unwrap();
        assert_eq!(looked_up, expected);
    }

    #[test]
    fn theme_token_switches_value_when_the_theme_switches() {
        let dark = quadraui::Theme::default();
        let light = quadraui::Theme::light_default();
        assert_ne!(
            theme_token(&dark, "background"),
            theme_token(&light, "background"),
            "the dark and light default palettes disagree on `background`, so the looked-up \
             token must disagree too"
        );
    }

    #[test]
    fn theme_token_returns_none_for_an_unknown_name() {
        let theme = quadraui::Theme::default();
        assert_eq!(theme_token(&theme, "not_a_real_field"), None);
    }

    #[test]
    fn style_token_looks_up_a_geometry_field_by_name() {
        let style = quadraui::Style::default();
        let looked_up =
            style_token(&style, "focus_ring_width").expect("focus_ring_width is a real field");
        assert_eq!(looked_up, serde_json::json!(style.focus_ring_width));
    }

    #[test]
    fn style_token_returns_none_for_an_unknown_name() {
        let style = quadraui::Style::default();
        assert_eq!(style_token(&style, "not_a_real_field"), None);
    }

    /// [`InteractionEngine::set_drag_roles`]/[`InteractionEngine::drag_roles`]'s
    /// whole contract: a fresh engine has none, `set_drag_roles` makes
    /// them readable back verbatim, and a later call replaces the table
    /// wholesale (an id dropped from the new call disappears, mirroring
    /// [`InteractionEngine::set_hits`]'s own "replaced every frame, not
    /// merged" lifecycle for [`InteractionEngine::hit_map`]).
    #[test]
    fn set_drag_roles_replaces_the_retained_table_wholesale_each_call() {
        use crate::wire::DragRole;

        let engine = InteractionEngine::new();
        assert!(
            engine.drag_roles().is_empty(),
            "a fresh engine has no drag roles until set_drag_roles is called"
        );

        let mut first = HashMap::new();
        first.insert(
            WidgetId::new("drag1"),
            DragRole {
                draggable: true,
                drop_target: false,
            },
        );
        first.insert(
            WidgetId::new("drop1"),
            DragRole {
                draggable: false,
                drop_target: true,
            },
        );
        engine.set_drag_roles(first);
        assert_eq!(
            engine.drag_roles().get(&WidgetId::new("drag1")),
            Some(&DragRole {
                draggable: true,
                drop_target: false,
            })
        );
        assert_eq!(engine.drag_roles().len(), 2);

        // A later call with only one entry replaces the table wholesale
        // -- "drop1" (and its own role) is gone, not merged alongside the
        // new entry.
        let mut second = HashMap::new();
        second.insert(
            WidgetId::new("drag2"),
            DragRole {
                draggable: true,
                drop_target: true,
            },
        );
        engine.set_drag_roles(second);
        assert_eq!(engine.drag_roles().len(), 1);
        assert_eq!(engine.drag_roles().get(&WidgetId::new("drop1")), None);
        assert_eq!(
            engine.drag_roles().get(&WidgetId::new("drag2")),
            Some(&DragRole {
                draggable: true,
                drop_target: true,
            })
        );
    }

    /// [`InteractionEngine::set_hits`] never touches
    /// [`InteractionEngine::drag_roles`] -- the two are sibling calls a
    /// binding makes independently, exactly like
    /// [`InteractionEngine::hit_map`] and `drag_roles` are sibling
    /// fields, not one derived from the other.
    #[test]
    fn set_hits_does_not_clear_drag_roles() {
        use crate::wire::DragRole;

        let engine = InteractionEngine::new();
        let mut roles = HashMap::new();
        roles.insert(
            WidgetId::new("drag1"),
            DragRole {
                draggable: true,
                drop_target: false,
            },
        );
        engine.set_drag_roles(roles);

        engine.set_hits(vec![PaintedHit::Widget {
            id: WidgetId::new("btn"),
            area: Rect::new(0.0, 0.0, 10.0, 1.0),
            widget: Rc::new(Button::new(WidgetId::new("btn"), "Button")),
        }]);

        assert_eq!(
            engine.drag_roles().get(&WidgetId::new("drag1")),
            Some(&DragRole {
                draggable: true,
                drop_target: false,
            }),
            "set_hits must not clear drag_roles -- a binding sets each independently"
        );
    }

    /// A `DynWidget` leaf with two focusable sub-parts. Every press asks
    /// [`EventHost::is_focused`] about sub-part 1 and about the leaf as a
    /// whole, logs both answers, then requests focus for sub-part 1.
    struct SubFocusProbe {
        id: WidgetId,
        log: Rc<RefCell<Vec<(bool, bool)>>>,
    }

    impl quadraui::A11yPrimitive for SubFocusProbe {
        fn a11y_info(&self) -> quadraui::A11yInfo {
            quadraui::A11yInfo::default()
        }
    }

    impl quadraui::Primitive for SubFocusProbe {
        type Layout = Rect;
        type Event = ();

        fn id(&self) -> Option<&WidgetId> {
            Some(&self.id)
        }

        fn layout(&self, _ctx: &quadraui::LayoutCtx, rect: Rect) -> Rect {
            rect
        }

        fn paint(
            &self,
            _layout: &Rect,
            _surface: &mut dyn quadraui::paint_surface::PaintSurface,
            _env: &quadraui::PaintEnv,
        ) {
        }

        fn paint_cells(
            &self,
            _layout: &Rect,
            _cells: &mut dyn quadraui::cell_surface::CellSurface,
            _env: &quadraui::PaintEnv,
        ) {
        }
    }

    impl quadraui::Interactive for SubFocusProbe {
        type State = ();
        type Action = quadraui::Never;

        fn focus(&self) -> quadraui::FocusSpec {
            quadraui::FocusSpec::Children(vec![
                quadraui::FocusStop {
                    sub: quadraui::interactive::SubTarget(0),
                },
                quadraui::FocusStop {
                    sub: quadraui::interactive::SubTarget(1),
                },
            ])
        }

        fn handle(
            &self,
            _state: &mut (),
            event: &quadraui::RoutedEvent,
            _layout: &Rect,
            cx: &mut quadraui::EventCx,
        ) -> quadraui::Handled<quadraui::Never> {
            if !matches!(
                event,
                quadraui::RoutedEvent::Pointer {
                    kind: quadraui::PointerKind::Down(_) | quadraui::PointerKind::DoubleClick(_),
                    ..
                }
            ) {
                return quadraui::Handled::Ignored;
            }
            let sub = quadraui::interactive::SubTarget(1);
            self.log
                .borrow_mut()
                .push((cx.is_focused(Some(sub)), cx.is_focused(None)));
            cx.request_focus(Some(sub));
            quadraui::Handled::Consumed
        }
    }

    struct SubFocusLogic {
        log: Rc<RefCell<Vec<(bool, bool)>>>,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for SubFocusLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let area = viewport_rect(backend);
            let content = LayoutNode::Leaf(LayoutLeaf {
                id: "probe".to_string(),
                descriptor: LeafDescriptor::Widget {
                    kind: "sub_focus_probe",
                    widget: Rc::new(SubFocusProbe {
                        id: WidgetId::new("probe"),
                        log: self.log.clone(),
                    }),
                },
                scroll_px: None,
            });
            let mut hits = Vec::new();
            paint_layout(backend, area, &content, &mut hits);
            render_overlays(&self.engine, backend, hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// A column holding an open `ComboBox` with a button laid out right
    /// under its field, where the open list drops.
    struct ComboOverButtonLogic {
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for ComboOverButtonLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let area = viewport_rect(backend);
            let mut combo = quadraui::ComboBox::new(
                WidgetId::new("combo"),
                ["alpha", "beta", "gamma", "delta"]
                    .map(String::from)
                    .to_vec(),
            );
            combo.open = true;
            let leaf = |id: &str, descriptor| {
                LayoutNode::Leaf(LayoutLeaf {
                    id: id.to_string(),
                    descriptor,
                    scroll_px: None,
                })
            };
            let mut node = LayoutNode::Column(LayoutContainer::new(
                vec![
                    leaf(
                        "combo",
                        LeafDescriptor::Widget {
                            kind: "combo_box",
                            widget: Rc::new(combo),
                        },
                    ),
                    leaf(
                        "under",
                        LeafDescriptor::Widget {
                            kind: "button",
                            widget: Rc::new(Button::new(WidgetId::new("under"), "under")),
                        },
                    ),
                ],
                Spacing::default(),
            ));
            resolve_layout_node(self.engine.widget_state(), backend, area, &mut node);
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// A press on an open `ComboBox` list never reaches the leaf the list
    /// covers.
    #[test]
    fn a_press_on_a_widget_popup_does_not_reach_the_leaf_beneath_it() {
        let logic = ComboOverButtonLogic {
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 30, 12);
        driver.render();
        let popup = quadraui::popup::painted_popups()
            .into_iter()
            .find(|p| p.owner == WidgetId::new("combo"))
            .expect("the open list is painted");
        let (x, y) = (popup.rect.x + 2.0, popup.rect.y + 1.0);
        assert!(driver.screen_contains("beta"), "{}", driver.screen());

        driver.click(x, y);

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            !dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::ButtonActivated { .. } | UiEvent::FocusChanged(Some(_))
            )),
            "{dispatched:?}"
        );
    }

    /// A leaf that requested focus for one of its own sub-parts sees that
    /// sub-part, and not the leaf as a whole, reported focused on its
    /// next event. The second press lands inside the double-click window,
    /// so it arrives as a `DoubleClick`.
    #[test]
    fn dyn_widget_host_reports_sub_target_focus() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let logic = SubFocusLogic {
            log: log.clone(),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();

        driver.click(5.0, 5.0);
        driver.render();
        driver.click(5.0, 5.0);
        driver.render();

        assert_eq!(
            *log.borrow(),
            vec![(false, false), (true, false)],
            "nothing is focused before the request; after requesting \
             sub-part 1 that sub-part, not the leaf as a whole, is focused"
        );
    }
}
