//! [`MessageDialog`] — the in-canvas message/alert box on the widget
//! contract.
//!
//! A `MessageDialog` composes the custom-drawn [`Dialog`]: it lays out
//! and paints through `Dialog`'s own [`Primitive`] and hands input to
//! `Dialog`'s [`Interactive`], adding only the alert-box keyboard model
//! on top. It gives the dialog the same show → block-until-resolved →
//! return-a-choice contract
//! [`crate::backend::PlatformServices::show_message_dialog`] has on a
//! backend with a native alert facility.
//!
//! On TUI, `TuiPlatformServices::show_message_dialog` drives the widget
//! through a nested draw-and-read loop (see `crate::tui::services` module
//! docs) to return a real [`crate::backend::MessageDialogChoice`] — see
//! `examples/tui_message_dialog.rs` for the widget hosted directly by an
//! app.
//!
//! # Hosting
//!
//! An app on the runtime's routing engine pushes the [`MessageDialog`]
//! itself as a `Surface::Widget` over the whole viewport and registers it
//! in its frame tree; the choice then arrives as
//! [`UiEvent::MessageDialogResolved`].
//!
//! A host that reads raw [`UiEvent`]s instead keeps the widget and its
//! [`MessageDialogState`], paints it with `draw_dyn` over the viewport and
//! resolves each event through [`MessageDialog::handle_event`]:
//!
//! ```rust,ignore
//! // Instantiate when the user triggers a confirm/alert:
//! let dialog = MessageDialog::new(MessageDialogOptions {
//!     title: "Unsaved Changes".to_string(),
//!     body: "Do you want to save?".to_string(),
//!     buttons: vec![
//!         MessageDialogButton { id: WidgetId::new("save"), label: "Save".into(), is_default: true, is_cancel: false, spec: None },
//!         MessageDialogButton { id: WidgetId::new("cancel"), label: "Cancel".into(), is_default: false, is_cancel: true, spec: None },
//!     ],
//!     severity: Some(DialogSeverity::Warning),
//! });
//! let mut state = MessageDialogState::default();
//!
//! // In AppLogic::render, whenever the dialog is open:
//! backend.draw_dyn(backend.viewport().rect(), &dialog, &());
//!
//! // In AppLogic::handle:
//! match dialog.handle_event(&mut state, &event, backend) {
//!     Handled::Event(choice) => { /* choice matches one of the MessageDialogButton::id values */ }
//!     Handled::Consumed => { /* redraw */ }
//!     Handled::Ignored => {}
//! }
//! ```
//!
//! # Keyboard model
//!
//! One focus stop per button. Left/Right and Tab/Shift+Tab cycle
//! keyboard focus among the buttons (wrapping); Enter activates the
//! focused button, which starts on the default one; Escape resolves to
//! [`Dialog::cancel_button_id`], falling back to
//! [`Dialog::default_button_id`] when no button declares `is_cancel` —
//! a single-button "OK" alert must still be dismissible with Escape, not
//! just Enter.
//!
//! # No visual keyboard-focus ring (known limitation)
//!
//! [`crate::primitives::dialog::DialogButton`] carries no
//! "keyboard-focused" field (unlike [`crate::primitives::toolbar::Toolbar::focused_index`]),
//! so the rendered dialog cannot visually distinguish the keyboard-focused
//! button from an unfocused one beyond `is_default`'s existing styling.
//! Enter/click routing is fully correct regardless — this only affects
//! what a sighted user sees highlighted while tabbing.

use crate::a11y::{A11yChild, A11yInfo, A11yPrimitive};
use crate::backend::DrawsPrimitives;
use crate::backend::{MessageDialogChoice, MessageDialogOptions};
use crate::compose::widget_host;
use crate::compose::Handled;
use crate::event::{Rect, UiEvent};
use crate::interactive::{
    EventCx, FocusSpec, Interactive, KeyEvent, RoutedEvent, SubTarget, WidgetAction,
};
use crate::primitive::{LayoutCtx, Primitive, Size};
use crate::primitives::dialog::{
    Dialog, DialogAction, DialogButton, DialogLayout, DialogPrimitiveLayout, DialogState,
};
use crate::types::{StyledText, WidgetId};
use crate::{Backend, Key, NamedKey};

// ── Widget ────────────────────────────────────────────────────────────────

/// The in-canvas message/alert box: a [`Dialog`] with the alert-box
/// keyboard model of this module's doc.
///
/// The modal centres itself in the rect it is laid out in, so a host
/// hands it the whole viewport.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageDialog {
    /// The dialog this alert paints and routes input through. Its
    /// [`Dialog::id`] is the widget's id.
    pub dialog: Dialog,
}

/// [`Interactive::State`] for [`MessageDialog`]: the composed
/// [`Dialog`]'s own state, which tracks the keyboard-focused button.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MessageDialogState {
    /// The composed dialog's state. Its `focused_button` stays `None`
    /// until a key or click moves focus, and the alert then reads its
    /// default button as the focused one.
    pub dialog: DialogState,
}

impl MessageDialog {
    /// Build an alert from the same [`MessageDialogOptions`]
    /// [`crate::backend::PlatformServices::show_message_dialog`] takes —
    /// so a TUI backend's implementation of that method can construct
    /// one directly from its `opts` argument with no translation step.
    ///
    /// `opts.body` is split on `\n` into one [`StyledText`] line per
    /// segment, matching [`Dialog::body`]'s multi-line shape (the inverse
    /// of what [`crate::primitives::dialog::native_dialog_options`] does
    /// when going the other way). The id defaults to
    /// `WidgetId::new("message_dialog")`.
    pub fn new(opts: MessageDialogOptions) -> Self {
        let buttons: Vec<DialogButton> = opts
            .buttons
            .iter()
            .map(|b| DialogButton {
                id: b.id,
                label: b.label.clone(),
                is_default: b.is_default,
                is_cancel: b.is_cancel,
                tint: None,
                spec: b.spec.clone(),
            })
            .collect();
        Self {
            dialog: Dialog {
                id: WidgetId::new("message_dialog"),
                title: StyledText::plain(opts.title),
                body: opts.body.lines().map(StyledText::plain).collect(),
                buttons,
                severity: opts.severity,
                vertical_buttons: false,
                table: None,
                input: None,
            },
        }
    }

    /// Override the alert's [`WidgetId`]. Returns `self` for
    /// builder-style chaining.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.dialog.id = WidgetId::new(id.into());
        self
    }

    /// Stack buttons vertically instead of the default right-aligned
    /// horizontal row — see [`Dialog::vertical_buttons`]. Returns `self`
    /// for builder-style chaining.
    pub fn with_vertical_buttons(mut self, vertical: bool) -> Self {
        self.dialog.vertical_buttons = vertical;
        self
    }

    /// The button keyboard focus sits on: the one a key or click moved
    /// it to, else the default button, else the first.
    pub fn focused_button(&self, state: &MessageDialogState) -> Option<usize> {
        if self.dialog.buttons.is_empty() {
            return None;
        }
        Some(
            state
                .dialog
                .focused_button
                .filter(|i| *i < self.dialog.buttons.len())
                .unwrap_or_else(|| {
                    self.dialog
                        .buttons
                        .iter()
                        .position(|b| b.is_default)
                        .unwrap_or(0)
                }),
        )
    }

    /// The `WidgetId` of the button keyboard focus sits on in `state` (see
    /// [`Self::focused_button`]).
    pub fn focused_button_id(&self, state: &MessageDialogState) -> Option<&WidgetId> {
        self.focused_button(state)
            .map(|i| &self.dialog.buttons[i].id)
    }

    /// The modal's geometry over `backend`'s viewport, resolved through
    /// the widget contract from the backend's own font metrics — the
    /// geometry a `draw_dyn` over the viewport's [`crate::Viewport::rect`]
    /// paints.
    pub fn dialog_layout(&self, backend: &dyn Backend) -> DialogLayout {
        backend
            .layout_primitive(backend.viewport().rect(), self)
            .layout
    }

    /// The dialog's full on-screen bounds over `backend`'s viewport. A
    /// host that tracks a [`crate::ModalStack`] entry for the open alert
    /// (so other modal-aware code gets the same hit-test precedence an
    /// open context menu has) pushes this.
    pub fn bounds(&self, backend: &dyn Backend) -> Rect {
        self.dialog_layout(backend).bounds
    }

    /// Resolve one raw key press or mouse press for a host that paints
    /// the alert over `backend`'s viewport, through the widget's own
    /// [`Interactive`] against `state`. Every other event is
    /// [`Handled::Ignored`]. A press anywhere that is not a button is
    /// [`Handled::Consumed`]: the alert is modal.
    pub fn handle_event(
        &self,
        state: &mut MessageDialogState,
        event: &UiEvent,
        backend: &dyn Backend,
    ) -> Handled<MessageDialogChoice> {
        if !matches!(
            event,
            UiEvent::KeyPressed { .. } | UiEvent::MouseDown { .. }
        ) {
            return Handled::Ignored;
        }
        match widget_host::drive_ui_event(self, state, event, backend, backend.viewport().rect()) {
            None => Handled::Ignored,
            Some(driven) => match (driven.handled, event) {
                (Handled::Ignored, UiEvent::MouseDown { .. }) => Handled::Consumed,
                (handled, _) => handled,
            },
        }
    }

    /// The choice Escape resolves to: the cancel button, else the default.
    fn escape_choice(&self) -> Option<MessageDialogChoice> {
        self.dialog
            .cancel_button_id()
            .or_else(|| self.dialog.default_button_id())
            .copied()
    }

    /// Move keyboard focus `delta` buttons along the row, wrapping.
    fn cycle(
        &self,
        state: &mut MessageDialogState,
        delta: isize,
        cx: &mut EventCx,
    ) -> Handled<MessageDialogChoice> {
        let n = self.dialog.buttons.len();
        let Some(current) = self.focused_button(state) else {
            return Handled::Ignored;
        };
        let next = crate::focus::cycle_index(current, delta, n);
        state.dialog.focused_button = Some(next);
        cx.request_focus(Some(SubTarget(next as u64)));
        cx.request_redraw();
        Handled::Consumed
    }

    /// The composed dialog's answer, as this alert's choice.
    fn choice_from(handled: Handled<DialogAction>) -> Handled<MessageDialogChoice> {
        match handled {
            Handled::Event(DialogAction::ButtonActivated(id)) => Handled::Event(id),
            Handled::Event(DialogAction::Cancelled { button: Some(id) }) => Handled::Event(id),
            Handled::Event(_) | Handled::Consumed => Handled::Consumed,
            Handled::Ignored => Handled::Ignored,
        }
    }
}

impl WidgetAction for MessageDialogChoice {
    /// A choice is the button the alert resolved to, which
    /// [`UiEvent::MessageDialogResolved`] carries — the crate's one "an
    /// in-canvas dialog was answered" channel.
    fn into_ui_event(self, _id: WidgetId) -> UiEvent {
        UiEvent::MessageDialogResolved(self)
    }
}

impl A11yPrimitive for MessageDialog {
    fn a11y_info(&self) -> A11yInfo {
        self.dialog.a11y_info()
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        self.dialog.a11y_children()
    }
}

impl Primitive for MessageDialog {
    type Layout = DialogPrimitiveLayout;
    type Event = MessageDialogChoice;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.dialog.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        Primitive::capture(&self.dialog)
    }

    fn paint_bleed(&self, style: &crate::Style) -> f32 {
        Primitive::paint_bleed(&self.dialog, style)
    }

    fn measure(&self, ctx: &LayoutCtx, avail: Size) -> crate::NaturalSize {
        Primitive::measure(&self.dialog, ctx, avail)
    }

    /// The composed dialog's own layout, centred in `rect`.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> DialogPrimitiveLayout {
        Primitive::layout(&self.dialog, ctx, rect)
    }

    fn paint(
        &self,
        layout: &DialogPrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        Primitive::paint(&self.dialog, layout, surface, env);
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &DialogPrimitiveLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        Primitive::paint_cells(&self.dialog, layout, cells, env);
    }
}

impl Interactive for MessageDialog {
    type State = MessageDialogState;
    type Action = MessageDialogChoice;

    /// The composed dialog's stops: one per button.
    fn focus(&self) -> FocusSpec {
        Interactive::focus(&self.dialog)
    }

    fn sub_regions(&self, layout: &DialogPrimitiveLayout) -> Vec<(Rect, SubTarget)> {
        Interactive::sub_regions(&self.dialog, layout)
    }

    fn a11y_actions(&self, state: &MessageDialogState) -> Vec<crate::a11y::A11yAction> {
        Interactive::a11y_actions(&self.dialog, &state.dialog)
    }

    /// Enter, Escape and the arrow/Tab keys follow this module's keyboard
    /// model; every other event — clicks, focus changes, assistive
    /// actions — is the composed dialog's, and its outcome is reported as
    /// the choice it names.
    fn handle(
        &self,
        state: &mut MessageDialogState,
        event: &RoutedEvent,
        layout: &DialogPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<MessageDialogChoice> {
        if let RoutedEvent::Key(KeyEvent { key, modifiers, .. }) = event {
            match key {
                Key::Named(NamedKey::Escape) => {
                    return match self.escape_choice() {
                        Some(id) => Handled::Event(id),
                        None => Handled::Ignored,
                    };
                }
                Key::Named(NamedKey::Enter) => {
                    return match self.focused_button(state) {
                        Some(i) => Handled::Event(self.dialog.buttons[i].id),
                        None => Handled::Ignored,
                    };
                }
                Key::Named(NamedKey::Left) => return self.cycle(state, -1, cx),
                Key::Named(NamedKey::Right) => return self.cycle(state, 1, cx),
                Key::Named(NamedKey::Tab) => {
                    return self.cycle(state, if modifiers.shift { -1 } else { 1 }, cx);
                }
                Key::Named(NamedKey::BackTab) => return self.cycle(state, -1, cx),
                _ => {}
            }
        }
        let handled = Interactive::handle(&self.dialog, &mut state.dialog, event, layout, cx);
        Self::choice_from(handled)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// `compose` compiles unconditionally (no `tui`/`gtk` feature gate on the
// module itself, `lib.rs`), but these tests need a concrete `Backend` impl
// to exercise `layout()`'s `backend.measure()`/`backend.viewport()` calls
// (`Dialog`'s layout is generic over real backend metrics, unlike
// `FolderPicker`, which hardcodes its own popup sizing and never
// needs one). `TuiBackend` is the cheapest concrete `Backend` in-tree, so
// this module is gated on the `tui` feature — every CI leg that runs tests
// enables it (`--features tui` and `--features gtk,tui`, see
// `CLAUDE.md`'s quality-gate commands), so this never loses coverage in
// practice.
#[cfg(all(test, feature = "tui"))]
mod tests {
    use super::*;
    use crate::backend::MessageDialogButton;
    use crate::event::Point;
    use crate::types::WidgetId;
    use crate::Modifiers;

    /// A raw-event host: the alert plus the state it keeps between events,
    /// driven through [`MessageDialog::handle_event`].
    struct Host {
        widget: MessageDialog,
        state: MessageDialogState,
    }

    impl Host {
        fn new(opts: MessageDialogOptions) -> Self {
            Self {
                widget: MessageDialog::new(opts),
                state: MessageDialogState::default(),
            }
        }

        fn with_id(mut self, id: &str) -> Self {
            self.widget = self.widget.with_id(id);
            self
        }

        fn with_vertical_buttons(mut self, vertical: bool) -> Self {
            self.widget = self.widget.with_vertical_buttons(vertical);
            self
        }

        fn dialog(&self) -> &Dialog {
            &self.widget.dialog
        }

        fn focused_button_id(&self) -> Option<&WidgetId> {
            self.widget.focused_button_id(&self.state)
        }

        fn layout(&self, backend: &dyn Backend) -> DialogLayout {
            self.widget.dialog_layout(backend)
        }

        fn bounds(&self, backend: &dyn Backend) -> Rect {
            self.widget.bounds(backend)
        }

        fn handle_event(
            &mut self,
            event: &UiEvent,
            backend: &dyn Backend,
        ) -> Handled<MessageDialogChoice> {
            self.widget.handle_event(&mut self.state, event, backend)
        }
    }

    fn opts_two_buttons() -> MessageDialogOptions {
        MessageDialogOptions {
            title: "Unsaved Changes".to_string(),
            body: "Do you want to save?\nChanges will be lost.".to_string(),
            buttons: vec![
                MessageDialogButton {
                    id: WidgetId::new("save"),
                    label: "Save".into(),
                    is_default: true,
                    is_cancel: false,
                    spec: None,
                },
                MessageDialogButton {
                    id: WidgetId::new("cancel"),
                    label: "Cancel".into(),
                    is_default: false,
                    is_cancel: true,
                    spec: None,
                },
            ],
            severity: Some(crate::primitives::dialog::DialogSeverity::Warning),
        }
    }

    fn opts_ok_only() -> MessageDialogOptions {
        MessageDialogOptions {
            title: "Heads up".to_string(),
            body: "Something happened.".to_string(),
            buttons: vec![MessageDialogButton {
                id: WidgetId::new("ok"),
                label: "OK".into(),
                is_default: true,
                is_cancel: false,
                spec: None,
            }],
            severity: None,
        }
    }

    #[test]
    fn new_splits_body_into_lines() {
        let host = Host::new(opts_two_buttons());
        assert_eq!(host.dialog().body.len(), 2);
    }

    #[test]
    fn new_focuses_default_button() {
        let host = Host::new(opts_two_buttons());
        assert_eq!(host.focused_button_id().unwrap().as_str(), "save");
    }

    #[test]
    fn new_focuses_first_button_when_no_default() {
        let mut opts = opts_two_buttons();
        opts.buttons[0].is_default = false;
        let host = Host::new(opts);
        assert_eq!(host.focused_button_id().unwrap().as_str(), "save");
    }

    #[test]
    fn enter_resolves_focused_button() {
        let mut host = Host::new(opts_two_buttons());
        let ev = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        // `backend` unused by Enter's branch — cheapest real Backend is
        // TuiBackend, gated behind the `tui` feature this crate's own
        // tests always build with.
        let backend = crate::tui::backend::TuiBackend::new();
        assert_eq!(
            host.handle_event(&ev, &backend),
            Handled::Event(WidgetId::new("save"))
        );
    }

    #[test]
    fn right_then_enter_resolves_second_button() {
        let mut host = Host::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        let right = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Right),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(host.handle_event(&right, &backend), Handled::Consumed);
        assert_eq!(host.focused_button_id().unwrap().as_str(), "cancel");
        let enter = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(
            host.handle_event(&enter, &backend),
            Handled::Event(WidgetId::new("cancel"))
        );
    }

    #[test]
    fn right_wraps_around() {
        let mut host = Host::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        let right = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Right),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let _ = host.handle_event(&right, &backend);
        let _ = host.handle_event(&right, &backend);
        assert_eq!(host.focused_button_id().unwrap().as_str(), "save");
    }

    #[test]
    fn left_wraps_around_from_first() {
        let mut host = Host::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        let left = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Left),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let _ = host.handle_event(&left, &backend);
        assert_eq!(host.focused_button_id().unwrap().as_str(), "cancel");
    }

    #[test]
    fn tab_cycles_focus_forward() {
        let mut host = Host::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        let tab = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let _ = host.handle_event(&tab, &backend);
        assert_eq!(host.focused_button_id().unwrap().as_str(), "cancel");
    }

    #[test]
    fn shift_tab_cycles_focus_backward() {
        let mut host = Host::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        let shift_tab = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
            repeat: false,
        };
        let _ = host.handle_event(&shift_tab, &backend);
        assert_eq!(host.focused_button_id().unwrap().as_str(), "cancel");
    }

    #[test]
    fn escape_resolves_cancel_button() {
        let mut host = Host::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        let esc = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(
            host.handle_event(&esc, &backend),
            Handled::Event(WidgetId::new("cancel"))
        );
    }

    #[test]
    fn escape_falls_back_to_default_when_no_cancel_button() {
        // A single "OK" button dialog has no `is_cancel` button — Escape
        // must still resolve (to the default), not hang the caller forever.
        let mut host = Host::new(opts_ok_only());
        let backend = crate::tui::backend::TuiBackend::new();
        let esc = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(
            host.handle_event(&esc, &backend),
            Handled::Event(WidgetId::new("ok"))
        );
    }

    #[test]
    fn click_on_button_resolves_it() {
        let host = Host::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        let layout = host.layout(&backend);
        let btn = &layout.visible_buttons[0];
        let cx = btn.bounds.x + 1.0;
        let cy = btn.bounds.y;
        let mut host = host;
        let ev = UiEvent::MouseDown {
            widget: None,
            button: crate::MouseButton::Left,
            position: Point::new(cx, cy),
            modifiers: Modifiers::default(),
        };
        let result = host.handle_event(&ev, &backend);
        assert!(matches!(result, Handled::Event(_)));
    }

    #[test]
    fn click_outside_buttons_is_consumed_not_ignored() {
        let mut host = Host::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        let ev = UiEvent::MouseDown {
            widget: None,
            button: crate::MouseButton::Left,
            position: Point::new(-100.0, -100.0),
            modifiers: Modifiers::default(),
        };
        assert_eq!(host.handle_event(&ev, &backend), Handled::Consumed);
    }

    /// A `MessageDialogButton::spec` override reaches the built `Dialog`'s
    /// `DialogButton::spec` unchanged — `new()` must not drop it the way
    /// it used to hardcode `spec: None` regardless of what `opts` carried.
    #[test]
    fn new_carries_button_spec_through_to_the_dialog() {
        let mut opts = opts_ok_only();
        let mut spec = crate::primitives::button::ButtonSpec::new("Move to Trash");
        spec.variant = crate::primitives::button::ButtonVariant::Danger;
        opts.buttons[0].spec = Some(spec.clone());

        let host = Host::new(opts);
        assert_eq!(host.dialog().buttons[0].spec, Some(spec));
    }

    /// A `MessageDialogButton` with no `spec` override still builds a
    /// `DialogButton` with `spec: None`, unaffected by this plumbing.
    #[test]
    fn new_leaves_spec_none_when_opts_set_none() {
        let host = Host::new(opts_ok_only());
        assert_eq!(host.dialog().buttons[0].spec, None);
    }

    #[test]
    fn with_id_overrides_widget_id() {
        let host = Host::new(opts_two_buttons()).with_id("confirm_close");
        assert_eq!(host.dialog().id.as_str(), "confirm_close");
    }

    #[test]
    fn bounds_matches_the_layout_render_and_handle_both_use() {
        let host = Host::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        assert_eq!(host.bounds(&backend), host.layout(&backend).bounds);
    }

    /// A `DialogButton` with an explicit `Danger` `spec` and a longer
    /// label than the flat `char_w * 10.0` floor gets a button box wide
    /// enough for `[ Move to Trash ]` to not clip — the same
    /// `ButtonSpec::min_cell_width` sizing `tui_dialog_layout` and
    /// `Dialog::measure_generic_inner` use.
    #[test]
    fn danger_spec_button_widens_the_button_row_so_it_is_not_clipped() {
        let mut host = Host::new(opts_ok_only());
        let mut spec = crate::primitives::button::ButtonSpec::new("Move to Trash");
        spec.variant = crate::primitives::button::ButtonVariant::Danger;
        host.widget.dialog.buttons[0].label = "Move to Trash".into();
        host.widget.dialog.buttons[0].spec = Some(spec.clone());

        let backend = crate::tui::backend::TuiBackend::new();
        let layout = host.layout(&backend);
        let vis = &layout.visible_buttons[0];
        let char_w = backend.measure().char_width;

        assert!(
            vis.bounds.width >= spec.min_cell_width() * char_w,
            "button box ({}) must be at least as wide as the spec needs ({})",
            vis.bounds.width,
            spec.min_cell_width() * char_w
        );
    }

    /// On a pixel backend, a short label ("OK") floors at the VS Code
    /// minimum button width rather than sizing down to just its own text
    /// + padding — otherwise a two-character label would paint narrower
    /// than the dialog's own undersized-pill bug this fix replaces.
    #[test]
    fn pixel_backend_short_label_button_floors_at_the_min_width() {
        let host = Host::new(opts_two_buttons());
        let backend = crate::testing::RecordingBackend {
            line_height: 16.0,
            char_width: 7.0,
            viewport: crate::Viewport {
                width: 800.0,
                height: 600.0,
                scale: 1.0,
            },
            ..Default::default()
        };

        let layout = host.layout(&backend);

        for vis in &layout.visible_buttons {
            assert!(
                vis.bounds.width >= 65.0,
                "a short-label button must not size below the VS Code \
                 minimum button width; got {}",
                vis.bounds.width,
            );
        }
    }

    /// On a pixel backend, a spec with an icon reserves room for the
    /// icon glyph and its gap before the label — mirrors
    /// `button::native_surface_paint::paint`'s own `icon_w + gap +
    /// label_w` content width, so the icon never clips against the
    /// label the way `danger_spec_button_widens_the_button_row_so_it_is_not_clipped`
    /// already pins on TUI.
    #[test]
    fn pixel_backend_icon_spec_button_widens_for_the_icon() {
        let long_label = "A Very Long Button Label Indeed";

        let mut host = Host::new(opts_ok_only());
        let mut spec = crate::primitives::button::ButtonSpec::new(long_label);
        spec.icon = Some(crate::types::Icon::new("\u{f071}", "!"));
        host.widget.dialog.buttons[0].label = long_label.into();
        host.widget.dialog.buttons[0].spec = Some(spec);

        let mut host_no_icon = Host::new(opts_ok_only());
        host_no_icon.widget.dialog.buttons[0].label = long_label.into();
        host_no_icon.widget.dialog.buttons[0].spec =
            Some(crate::primitives::button::ButtonSpec::new(long_label));

        let backend = crate::testing::RecordingBackend {
            line_height: 16.0,
            char_width: 7.0,
            viewport: crate::Viewport {
                width: 800.0,
                height: 600.0,
                scale: 1.0,
            },
            ..Default::default()
        };

        let with_icon = host.layout(&backend).visible_buttons[0].bounds.width;
        let without_icon = host_no_icon.layout(&backend).visible_buttons[0]
            .bounds
            .width;

        assert!(
            with_icon > without_icon,
            "an icon spec must widen the button beyond the plain-label \
             width; with_icon={with_icon}, without_icon={without_icon}",
        );
    }

    /// On a pixel backend (`line_height > 1.0`), the button row height is
    /// `Style::control_height` — the VS Code-matching 26px row, not the
    /// chrome font's own (usually shorter) line height — the
    /// control_height row this test pins.
    #[test]
    fn pixel_backend_button_row_height_matches_control_height() {
        let host = Host::new(opts_two_buttons());
        let backend = crate::testing::RecordingBackend {
            line_height: 16.0,
            char_width: 7.0,
            viewport: crate::Viewport {
                width: 800.0,
                height: 600.0,
                scale: 1.0,
            },
            ..Default::default()
        };

        let layout = host.layout(&backend);

        assert_eq!(layout.visible_buttons.len(), 2);
        for vis in &layout.visible_buttons {
            assert_eq!(vis.bounds.height, crate::Style::default().control_height);
        }
    }

    /// Same as `pixel_backend_button_row_height_matches_control_height`,
    /// but through a `Backend::set_style` override — pins that the pixel
    /// path reads the *live* `Style`, not a hardcoded `26.0` literal.
    #[test]
    fn pixel_backend_button_row_height_follows_a_set_style_override() {
        let host = Host::new(opts_two_buttons());
        let mut backend = crate::testing::RecordingBackend {
            line_height: 16.0,
            char_width: 7.0,
            viewport: crate::Viewport {
                width: 800.0,
                height: 600.0,
                scale: 1.0,
            },
            ..Default::default()
        };
        backend.set_style(crate::Style::default().with_control_height(40.0));

        let layout = host.layout(&backend);

        assert_eq!(layout.visible_buttons[0].bounds.height, 40.0);
    }

    /// TUI's cell-grid geometry is unchanged by the pixel-path fix above
    /// — `line_height <= 1.0` keeps sizing the button row off the raw
    /// chrome line height, same as before.
    #[test]
    fn tui_backend_button_row_height_stays_the_raw_line_height() {
        let host = Host::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();

        let layout = host.layout(&backend);

        let lh = backend.measure().line_height;
        assert!(lh <= 1.0, "TUI's line_height should stay cell-shaped");
        for vis in &layout.visible_buttons {
            assert_eq!(vis.bounds.height, lh);
        }
    }

    #[test]
    fn with_vertical_buttons_sets_flag() {
        let host = Host::new(opts_two_buttons()).with_vertical_buttons(true);
        assert!(host.dialog().vertical_buttons);
    }

    #[test]
    fn other_events_are_ignored() {
        let mut host = Host::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        let ev = UiEvent::CharTyped('x');
        assert_eq!(host.handle_event(&ev, &backend), Handled::Ignored);
    }
}
