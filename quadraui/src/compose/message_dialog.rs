//! `MessageDialogController` — engine-level message/alert box driver.
//!
//! Owns the interaction state (keyboard button focus, resolve-on-click)
//! for the in-canvas [`Dialog`] primitive, giving it the same
//! show → block-until-resolved → return-a-choice contract
//! [`crate::backend::PlatformServices::show_message_dialog`] has on a
//! backend with a native alert facility. Renders via the existing
//! `Dialog` primitive ([`Backend::draw_dialog`]) — no new `Backend`
//! trait method needed, mirroring [`crate::compose::FolderPickerController`].
//!
//! # Relation to issue #965
//!
//! Before this controller existed, `TuiPlatformServices::show_message_dialog`
//! returned `None` unconditionally — a terminal has no native alert
//! facility, and nothing in this crate drove the `Dialog` primitive's
//! show/block/resolve contract to degrade to. `crate::tui::services`
//! now drives exactly this controller through a nested draw-and-read
//! loop (see that module's doc) so `show_message_dialog` returns a real
//! [`crate::backend::MessageDialogChoice`] on TUI too — see
//! `examples/tui_message_dialog.rs` for the same controller used
//! directly by an app (no `PlatformServices` involved), and
//! `crate::tui::services`'s own tests for the `PlatformServices`-level
//! round trip.
//!
//! # Usage pattern
//!
//! ```rust,ignore
//! // Instantiate when the user triggers a confirm/alert:
//! let mut dialog = MessageDialogController::new(MessageDialogOptions {
//!     title: "Unsaved Changes".to_string(),
//!     body: "Do you want to save?".to_string(),
//!     buttons: vec![
//!         MessageDialogButton { id: WidgetId::new("save"), label: "Save".into(), is_default: true, is_cancel: false },
//!         MessageDialogButton { id: WidgetId::new("cancel"), label: "Cancel".into(), is_default: false, is_cancel: true },
//!     ],
//!     severity: Some(DialogSeverity::Warning),
//! });
//!
//! // In AppLogic::render, whenever the dialog is open:
//! dialog.render(backend);
//!
//! // In AppLogic::handle:
//! match dialog.handle(&event, backend) {
//!     MessageDialogEvent::Resolved(id) => { /* id matches one of the MessageDialogButton::id values */ }
//!     MessageDialogEvent::Consumed => { /* redraw */ }
//!     MessageDialogEvent::Ignored => {}
//! }
//! ```
//!
//! # Keyboard model
//!
//! Left/Right and Tab/Shift+Tab cycle keyboard focus among the buttons
//! (wrapping); Enter activates the focused button; Escape resolves to
//! [`Dialog::cancel_button_id`], falling back to
//! [`Dialog::default_button_id`] when no button declares
//! `is_cancel` — the same "there's always something to degrade to"
//! principle this controller exists to uphold (a single-button "OK"
//! alert must still be dismissible with Escape, not just Enter).
//!
//! # No visual keyboard-focus ring (known limitation)
//!
//! [`crate::primitives::dialog::DialogButton`] carries no
//! "keyboard-focused" field (unlike [`crate::primitives::toolbar::Toolbar::focused_index`]),
//! so the rendered dialog cannot visually distinguish the keyboard-focused
//! button from an unfocused one beyond `is_default`'s existing styling.
//! Enter/click routing is fully correct regardless — this only affects
//! what a sighted user sees highlighted while tabbing. Extending
//! `DialogButton` to carry focus is a separate, primitives-layer change
//! (would touch every backend's `draw_dialog` rasteriser), out of scope
//! here.

use crate::backend::{MessageDialogChoice, MessageDialogOptions};
use crate::event::{Point, Rect, UiEvent};
use crate::primitives::dialog::{Dialog, DialogButton, DialogHit, DialogLayout, DialogMeasure};
use crate::primitives::toolbar::ToolbarItemMeasure;
use crate::types::StyledText;
use crate::{Backend, Key, Modifiers, NamedKey};

/// What happened after [`MessageDialogController::handle`] processed an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageDialogEvent {
    /// The user resolved the dialog — via Enter, a click on a button, or
    /// Escape resolving to the cancel/default button. The consumer should
    /// close the dialog and act on `choice`, matching it against the
    /// `id` of the [`crate::backend::MessageDialogButton`] it built the
    /// dialog from.
    Resolved(MessageDialogChoice),
    /// Event consumed — internal state (keyboard focus) changed, caller
    /// should redraw.
    Consumed,
    /// Event not relevant to this controller.
    Ignored,
}

/// Cross-backend compose controller for the in-canvas message/alert box.
///
/// See the [module-level documentation](self) for a usage example.
pub struct MessageDialogController {
    dialog: Dialog,
    /// Index into `dialog.buttons` of the keyboard-focused button.
    /// Seeded from [`Dialog::default_button_id`] (or `0` when there is
    /// no default and at least one button).
    focused: usize,
}

impl MessageDialogController {
    /// Build a controller from the same [`MessageDialogOptions`]
    /// [`crate::backend::PlatformServices::show_message_dialog`] takes —
    /// so a TUI backend's implementation of that method can construct
    /// one directly from its `opts` argument with no translation step.
    ///
    /// `opts.body` is split on `\n` into one [`crate::types::StyledText`]
    /// line per segment, matching [`crate::primitives::dialog::Dialog::body`]'s
    /// multi-line shape (the inverse of what
    /// [`crate::primitives::dialog::native_dialog_options`] does when
    /// going the other way).
    pub fn new(opts: MessageDialogOptions) -> Self {
        let buttons: Vec<DialogButton> = opts
            .buttons
            .iter()
            .map(|b| DialogButton {
                id: b.id.clone(),
                label: b.label.clone(),
                is_default: b.is_default,
                is_cancel: b.is_cancel,
                tint: None,
            })
            .collect();
        let focused = buttons.iter().position(|b| b.is_default).unwrap_or(0);
        let dialog = Dialog {
            id: crate::types::WidgetId::new("message_dialog"),
            title: StyledText::plain(opts.title),
            body: opts.body.lines().map(StyledText::plain).collect(),
            buttons,
            severity: opts.severity,
            vertical_buttons: false,
            table: None,
            input: None,
        };
        Self { dialog, focused }
    }

    /// Override the [`crate::types::WidgetId`] used by the rendered
    /// `Dialog`. Defaults to `WidgetId::new("message_dialog")`. Returns
    /// `self` for builder-style chaining.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.dialog.id = crate::types::WidgetId::new(id.into());
        self
    }

    /// Stack buttons vertically instead of the default right-aligned
    /// horizontal row — useful for narrow viewports or many-choice
    /// dialogs. See [`Dialog::vertical_buttons`]. Returns `self` for
    /// builder-style chaining.
    pub fn with_vertical_buttons(mut self, vertical: bool) -> Self {
        self.dialog.vertical_buttons = vertical;
        self
    }

    /// The underlying [`Dialog`] descriptor.
    pub fn dialog(&self) -> &Dialog {
        &self.dialog
    }

    /// The `WidgetId` of the currently keyboard-focused button, if any.
    pub fn focused_button_id(&self) -> Option<&crate::types::WidgetId> {
        self.dialog.buttons.get(self.focused).map(|b| &b.id)
    }

    // ── Layout ────────────────────────────────────────────────────────

    /// Compute the dialog layout from the backend's own metrics
    /// ([`Backend::measure`]/[`Backend::viewport`]) — the "one layout fn,
    /// two callers" pattern `docs/LESSONS.md` documents (also used by
    /// `examples/common/modal_occlusion_demo.rs`), so paint and hit-test
    /// can never disagree.
    fn layout(&self, backend: &dyn Backend) -> DialogLayout {
        let m = backend.measure();
        let lh = m.line_height;
        let char_w = m.char_width;
        let viewport = backend.viewport();
        let has_title = self.dialog.title.spans.iter().any(|s| !s.text.is_empty());
        // `button_row_height` is a *single* row; `Dialog::layout` itself
        // multiplies by `self.buttons.len()` when `vertical_buttons` is
        // set (see that method's own `button_block_h` local) — no need
        // to pre-multiply here.
        let measure = DialogMeasure {
            width: (viewport.width * 0.5).clamp(char_w * 24.0, char_w * 64.0),
            title_height: if has_title { lh } else { 0.0 },
            body_height: lh * self.dialog.body.len().max(1) as f32,
            table_height: 0.0,
            input_height: 0.0,
            button_row_height: lh,
            button_width: char_w * 10.0,
            button_gap: char_w * 2.0,
            padding: lh,
        };
        let viewport_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height);
        self.dialog
            .layout(viewport_rect, measure, |_| ToolbarItemMeasure::new(0.0))
    }

    // ── Render ────────────────────────────────────────────────────────

    /// Paint the dialog, centered in the backend's current viewport.
    ///
    /// Call this from `AppLogic::render` whenever the dialog is open.
    pub fn render(&self, backend: &mut dyn Backend) {
        let layout = self.layout(backend);
        backend.draw_dialog(&self.dialog, &layout);
    }

    // ── Handle ────────────────────────────────────────────────────────

    /// Drive the state machine with a backend-neutral [`UiEvent`].
    ///
    /// Needs `backend` (immutably) to recompute the same [`DialogLayout`]
    /// [`Self::render`] just painted, so a `MouseDown` can hit-test
    /// against the exact button bounds on screen.
    pub fn handle(&mut self, event: &UiEvent, backend: &dyn Backend) -> MessageDialogEvent {
        match event {
            UiEvent::KeyPressed { key, modifiers, .. } => self.handle_key(key, modifiers),
            UiEvent::MouseDown { position, .. } => self.handle_click(*position, backend),
            _ => MessageDialogEvent::Ignored,
        }
    }

    fn handle_key(&mut self, key: &Key, modifiers: &Modifiers) -> MessageDialogEvent {
        let n = self.dialog.buttons.len();
        match key {
            Key::Named(NamedKey::Escape) => {
                match self
                    .dialog
                    .cancel_button_id()
                    .or_else(|| self.dialog.default_button_id())
                {
                    Some(id) => MessageDialogEvent::Resolved(id.clone()),
                    None => MessageDialogEvent::Ignored,
                }
            }
            Key::Named(NamedKey::Enter) => match self.dialog.buttons.get(self.focused) {
                Some(b) => MessageDialogEvent::Resolved(b.id.clone()),
                None => MessageDialogEvent::Ignored,
            },
            Key::Named(NamedKey::Left) if n > 0 => {
                self.focused = (self.focused + n - 1) % n;
                MessageDialogEvent::Consumed
            }
            Key::Named(NamedKey::Right) if n > 0 => {
                self.focused = (self.focused + 1) % n;
                MessageDialogEvent::Consumed
            }
            Key::Named(NamedKey::Tab) if n > 0 => {
                self.focused = if modifiers.shift {
                    (self.focused + n - 1) % n
                } else {
                    (self.focused + 1) % n
                };
                MessageDialogEvent::Consumed
            }
            _ => MessageDialogEvent::Ignored,
        }
    }

    fn handle_click(&mut self, position: Point, backend: &dyn Backend) -> MessageDialogEvent {
        let layout = self.layout(backend);
        match layout.hit_test(position.x, position.y) {
            DialogHit::Button(id) => MessageDialogEvent::Resolved(id),
            // Modal overlay: swallow clicks on the body or outside rather
            // than letting them fall through, matching `Dialog`'s own
            // backend contract ("Modal overlay — intercept all clicks").
            _ => MessageDialogEvent::Consumed,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// `compose` compiles unconditionally (no `tui`/`gtk` feature gate on the
// module itself, `lib.rs`), but these tests need a concrete `Backend` impl
// to exercise `layout()`'s `backend.measure()`/`backend.viewport()` calls
// (`Dialog`'s layout is generic over real backend metrics, unlike
// `FolderPickerController`, which hardcodes its own popup sizing and never
// needs one). `TuiBackend` is the cheapest concrete `Backend` in-tree, so
// this module is gated on the `tui` feature — every CI leg that runs tests
// enables it (`--features tui` and `--features gtk,tui`, see
// `CLAUDE.md`'s quality-gate commands), so this never loses coverage in
// practice.
#[cfg(all(test, feature = "tui"))]
mod tests {
    use super::*;
    use crate::backend::MessageDialogButton;
    use crate::types::WidgetId;

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
                },
                MessageDialogButton {
                    id: WidgetId::new("cancel"),
                    label: "Cancel".into(),
                    is_default: false,
                    is_cancel: true,
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
            }],
            severity: None,
        }
    }

    #[test]
    fn new_splits_body_into_lines() {
        let controller = MessageDialogController::new(opts_two_buttons());
        assert_eq!(controller.dialog().body.len(), 2);
    }

    #[test]
    fn new_focuses_default_button() {
        let controller = MessageDialogController::new(opts_two_buttons());
        assert_eq!(controller.focused_button_id().unwrap().as_str(), "save");
    }

    #[test]
    fn new_focuses_first_button_when_no_default() {
        let mut opts = opts_two_buttons();
        opts.buttons[0].is_default = false;
        let controller = MessageDialogController::new(opts);
        assert_eq!(controller.focused_button_id().unwrap().as_str(), "save");
    }

    #[test]
    fn enter_resolves_focused_button() {
        let mut controller = MessageDialogController::new(opts_two_buttons());
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
            controller.handle(&ev, &backend),
            MessageDialogEvent::Resolved(WidgetId::new("save"))
        );
    }

    #[test]
    fn right_then_enter_resolves_second_button() {
        let mut controller = MessageDialogController::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        let right = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Right),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(
            controller.handle(&right, &backend),
            MessageDialogEvent::Consumed
        );
        assert_eq!(controller.focused_button_id().unwrap().as_str(), "cancel");
        let enter = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(
            controller.handle(&enter, &backend),
            MessageDialogEvent::Resolved(WidgetId::new("cancel"))
        );
    }

    #[test]
    fn right_wraps_around() {
        let mut controller = MessageDialogController::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        let right = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Right),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        controller.handle(&right, &backend);
        controller.handle(&right, &backend);
        assert_eq!(controller.focused_button_id().unwrap().as_str(), "save");
    }

    #[test]
    fn left_wraps_around_from_first() {
        let mut controller = MessageDialogController::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        let left = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Left),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        controller.handle(&left, &backend);
        assert_eq!(controller.focused_button_id().unwrap().as_str(), "cancel");
    }

    #[test]
    fn tab_cycles_focus_forward() {
        let mut controller = MessageDialogController::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        let tab = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        controller.handle(&tab, &backend);
        assert_eq!(controller.focused_button_id().unwrap().as_str(), "cancel");
    }

    #[test]
    fn shift_tab_cycles_focus_backward() {
        let mut controller = MessageDialogController::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        let shift_tab = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
            repeat: false,
        };
        controller.handle(&shift_tab, &backend);
        assert_eq!(controller.focused_button_id().unwrap().as_str(), "cancel");
    }

    #[test]
    fn escape_resolves_cancel_button() {
        let mut controller = MessageDialogController::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        let esc = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(
            controller.handle(&esc, &backend),
            MessageDialogEvent::Resolved(WidgetId::new("cancel"))
        );
    }

    #[test]
    fn escape_falls_back_to_default_when_no_cancel_button() {
        // A single "OK" button dialog has no `is_cancel` button — Escape
        // must still resolve (to the default), not hang the caller
        // forever. This is the concrete case #965's "there is always
        // something to degrade to" principle protects against.
        let mut controller = MessageDialogController::new(opts_ok_only());
        let backend = crate::tui::backend::TuiBackend::new();
        let esc = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(
            controller.handle(&esc, &backend),
            MessageDialogEvent::Resolved(WidgetId::new("ok"))
        );
    }

    #[test]
    fn click_on_button_resolves_it() {
        let controller = MessageDialogController::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        let layout = controller.layout(&backend);
        let btn = &layout.visible_buttons[0];
        let cx = btn.bounds.x + 1.0;
        let cy = btn.bounds.y;
        let mut controller = controller;
        let ev = UiEvent::MouseDown {
            widget: None,
            button: crate::MouseButton::Left,
            position: Point::new(cx, cy),
            modifiers: Modifiers::default(),
        };
        let result = controller.handle(&ev, &backend);
        assert!(matches!(result, MessageDialogEvent::Resolved(_)));
    }

    #[test]
    fn click_outside_buttons_is_consumed_not_ignored() {
        let mut controller = MessageDialogController::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        let ev = UiEvent::MouseDown {
            widget: None,
            button: crate::MouseButton::Left,
            position: Point::new(-100.0, -100.0),
            modifiers: Modifiers::default(),
        };
        assert_eq!(
            controller.handle(&ev, &backend),
            MessageDialogEvent::Consumed
        );
    }

    #[test]
    fn with_id_overrides_widget_id() {
        let controller = MessageDialogController::new(opts_two_buttons()).with_id("confirm_close");
        assert_eq!(controller.dialog().id.as_str(), "confirm_close");
    }

    #[test]
    fn with_vertical_buttons_sets_flag() {
        let controller =
            MessageDialogController::new(opts_two_buttons()).with_vertical_buttons(true);
        assert!(controller.dialog().vertical_buttons);
    }

    #[test]
    fn other_events_are_ignored() {
        let mut controller = MessageDialogController::new(opts_two_buttons());
        let backend = crate::tui::backend::TuiBackend::new();
        let ev = UiEvent::CharTyped('x');
        assert_eq!(
            controller.handle(&ev, &backend),
            MessageDialogEvent::Ignored
        );
    }
}
