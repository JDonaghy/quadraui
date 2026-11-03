//! In-canvas overlays (context menu, message dialog, toast, tooltip,
//! completions, float, rich-text popup) -- thin napi-rs leaves straight
//! over [`InteractionEngine`]'s own anchored-overlay surface, the napi-rs
//! analog of `quadraui-py`'s own overlay methods on `QuiApp`.
//!
//! Every method below follows `quadraui-py`'s own shape: decode a plain JS
//! object straight into the matching `quadraui` primitive, route it onto
//! `InteractionEngine`'s own anchored-overlay slot, re-render immediately
//! so `screen()`/`screenContains()` reflect the open overlay without a
//! separate step. Call each from top-level script code only, never from
//! inside the registered event handler or render callback -- both already
//! hold this `QuiApp`'s one mutable borrow for the whole dispatch, so a
//! reentrant call here raises `ReentrantCallError` the same way
//! `insertButton`/`slotClear` do (see the module doc's "cannot call back
//! into `QuiApp`" note). From inside a handler, call the matching
//! [`QuiAppActions`] method (via `QuiApp::actions()`) instead.

use super::*;

#[napi]
impl QuiApp {
    /// Opens a context menu built from `items` at `position` (an `{x, y}`
    /// object) -- [`InteractionEngine::open_context_menu`]. Selecting an
    /// item surfaces as `UiEvent::ContextMenuItemActivated`; dismissing
    /// with Escape or a click outside surfaces as
    /// `UiEvent::ContextMenuDismissed`, both through the normal registered
    /// handler. Calling this again while a popup from an earlier call is
    /// still open replaces it with the new one silently.
    #[napi(js_name = "showContextMenu")]
    pub fn show_context_menu(&mut self, env: Env, items: Unknown, position: Unknown) -> Result<()> {
        let items: Vec<ContextMenuItem> = decode_direct(env, "showContextMenu", items)?;
        let position: Point = decode_direct(env, "showContextMenu", position)?;
        self.guarded(env, |app| {
            let (backend, app_logic) = app.driver.parts_mut();
            app_logic.engine.open_context_menu(items, position, backend);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(())
        })
    }

    /// Opens an in-canvas confirm/alert/prompt dialog built from `title`/
    /// `body`/`buttons`/`severity` -- [`InteractionEngine::open_message_dialog`].
    /// `buttons` is a non-empty array of objects shaped like the in-canvas
    /// `Dialog` primitive's own `buttons` field. `severity` is one of
    /// `"info"`, `"question"`, `"warning"`, `"error"`, or `undefined`/`null`
    /// for no severity tint. Calling this again while a dialog from an
    /// earlier call is still open replaces it with the new one silently;
    /// call `dismissMessageDialog` to close without resolving.
    #[napi(js_name = "showMessageDialog")]
    pub fn show_message_dialog(
        &mut self,
        env: Env,
        title: String,
        body: String,
        buttons: Unknown,
        severity: Option<String>,
    ) -> Result<()> {
        let buttons: Vec<DialogButton> = decode_direct(env, "showMessageDialog", buttons)?;
        let severity = severity
            .map(|s| {
                dialog_severity(&s)
                    .map_err(|err| invalid_descriptor_error(format!("showMessageDialog: {err}")))
            })
            .transpose()?;
        let opts = message_dialog_options_from_buttons(title, body, buttons, severity)
            .map_err(|err| invalid_descriptor_error(format!("showMessageDialog: {err}")))?;
        self.guarded(env, |app| {
            let (backend, app_logic) = app.driver.parts_mut();
            app_logic.engine.open_message_dialog(opts, backend);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(())
        })
    }

    /// Closes the open message dialog (if any) without resolving it --
    /// `UiEvent::MessageDialogResolved` does not fire. Returns whether a
    /// dialog was open to close.
    #[napi(js_name = "dismissMessageDialog")]
    pub fn dismiss_message_dialog(&mut self, env: Env) -> Result<bool> {
        self.guarded(env, |app| {
            let (backend, app_logic) = app.driver.parts_mut();
            let closed = app_logic.engine.close_message_dialog(backend);
            if closed {
                app.driver.render();
                take_pending_error(app.driver.app_mut())?;
            }
            Ok(closed)
        })
    }

    /// Shows `toast` (an object matching [`Toast`]'s shape) stacked in
    /// `corner` (one of `"bottom_right"` (default), `"bottom_left"`,
    /// `"top_right"`, `"top_left"`) among whatever toasts are already
    /// showing -- [`InteractionEngine::show_toast`]. Replaces any existing
    /// toast with the same `id`. A click on an action button dispatches
    /// `UiEvent::ToastActionClicked(id)`; a click on the dismiss button
    /// closes it directly with no event.
    #[napi(js_name = "showToast")]
    pub fn show_toast(&mut self, env: Env, toast: Unknown, corner: Option<String>) -> Result<()> {
        let toast: Toast = decode_direct(env, "showToast", toast)?;
        let corner = corner
            .map(|c| {
                toast_corner(&c)
                    .map_err(|err| invalid_descriptor_error(format!("showToast: {err}")))
            })
            .transpose()?
            .unwrap_or_default();
        self.guarded(env, |app| {
            app.driver.app_mut().engine.show_toast(toast, corner);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(())
        })
    }

    /// Closes the toast with this id, if showing -- no event is dispatched.
    /// Returns whether one was closed.
    #[napi(js_name = "dismissToast")]
    pub fn dismiss_toast(&mut self, env: Env, id: String) -> Result<bool> {
        self.guarded(env, |app| {
            let closed = app.driver.app_mut().engine.dismiss_toast(WidgetId::new(id));
            if closed {
                app.driver.render();
                take_pending_error(app.driver.app_mut())?;
            }
            Ok(closed)
        })
    }

    /// Shows `tooltip` (an object matching [`quadraui::Tooltip`]'s shape)
    /// anchored against `anchor`, an `{x, y, width, height}` rect in this
    /// app's own cell grid -- [`InteractionEngine::show_tooltip`]. Replaces
    /// any tooltip already showing. The next `MouseDown`/`KeyPressed` this
    /// app handles auto-dismisses it; call `hideTooltip` to dismiss sooner.
    #[napi(js_name = "showTooltip")]
    pub fn show_tooltip(&mut self, env: Env, tooltip: Unknown, anchor: Unknown) -> Result<()> {
        let tooltip: quadraui::Tooltip = decode_direct(env, "showTooltip", tooltip)?;
        let anchor: Rect = decode_direct(env, "showTooltip", anchor)?;
        self.guarded(env, |app| {
            app.driver.app_mut().engine.show_tooltip(tooltip, anchor);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(())
        })
    }

    /// Hides the open tooltip (if any). Returns whether one was open.
    #[napi(js_name = "hideTooltip")]
    pub fn hide_tooltip(&mut self, env: Env) -> Result<bool> {
        self.guarded(env, |app| {
            let hidden = app.driver.app_mut().engine.hide_tooltip();
            if hidden {
                app.driver.render();
                take_pending_error(app.driver.app_mut())?;
            }
            Ok(hidden)
        })
    }

    /// Shows `completions` (an object matching [`quadraui::Completions`]'s
    /// shape) anchored at `cursor`, an `{x, y}` caret position in this
    /// app's own cell grid -- [`InteractionEngine::show_completions`].
    /// Replaces any popup already open. A click on an item dispatches
    /// `UiEvent::CompletionItemActivated(id, index)` and closes the popup;
    /// Escape, or a click outside it, closes it with no event.
    #[napi(js_name = "showCompletions")]
    pub fn show_completions(
        &mut self,
        env: Env,
        completions: Unknown,
        cursor: Unknown,
    ) -> Result<()> {
        let completions: quadraui::Completions =
            decode_direct(env, "showCompletions", completions)?;
        let cursor: Point = decode_direct(env, "showCompletions", cursor)?;
        self.guarded(env, |app| {
            app.driver
                .app_mut()
                .engine
                .show_completions(completions, cursor);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(())
        })
    }

    /// Closes the open completions popup (if any) without reporting a
    /// selection. Returns whether one was open.
    #[napi(js_name = "dismissCompletions")]
    pub fn dismiss_completions(&mut self, env: Env) -> Result<bool> {
        self.guarded(env, |app| {
            let closed = app.driver.app_mut().engine.dismiss_completions();
            if closed {
                app.driver.render();
                take_pending_error(app.driver.app_mut())?;
            }
            Ok(closed)
        })
    }

    /// Opens `float` (an object matching [`quadraui::Float`]'s shape) at
    /// `width`x`height` cells -- [`InteractionEngine::show_float`]. Chrome
    /// only -- paints the float's background/border, nothing else; there
    /// is no content surface on this binding yet. Escape closes a
    /// focusable float (`float.focusable`, `true` by default) with no
    /// event; a non-focusable hint float never claims Escape.
    #[napi(js_name = "showFloat")]
    pub fn show_float(&mut self, env: Env, float: Unknown, width: f64, height: f64) -> Result<()> {
        let float: quadraui::Float = decode_direct(env, "showFloat", float)?;
        let measure = FloatMeasure::new(width as f32, height as f32);
        self.guarded(env, |app| {
            let (backend, app_logic) = app.driver.parts_mut();
            app_logic.engine.show_float(float, measure, backend);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(())
        })
    }

    /// Closes the open float (if any) with no event. Returns whether one
    /// was open.
    #[napi(js_name = "dismissFloat")]
    pub fn dismiss_float(&mut self, env: Env) -> Result<bool> {
        self.guarded(env, |app| {
            let (backend, app_logic) = app.driver.parts_mut();
            let closed = app_logic.engine.dismiss_float(backend);
            if closed {
                app.driver.render();
                take_pending_error(app.driver.app_mut())?;
            }
            Ok(closed)
        })
    }

    /// Shows `popup` (an object matching [`quadraui::RichTextPopup`]'s
    /// shape) anchored at `anchor`, an `{x, y}` position in this app's own
    /// cell grid -- [`InteractionEngine::show_rich_text_popup`]. Replaces
    /// any popup already open. A click on a link dispatches
    /// `UiEvent::RichTextLinkClicked(id, url)`; Escape, or a click outside
    /// the popup, closes it with no event.
    #[napi(js_name = "showRichTextPopup")]
    pub fn show_rich_text_popup(
        &mut self,
        env: Env,
        popup: Unknown,
        anchor: Unknown,
    ) -> Result<()> {
        let popup: quadraui::RichTextPopup = decode_direct(env, "showRichTextPopup", popup)?;
        let anchor: Point = decode_direct(env, "showRichTextPopup", anchor)?;
        self.guarded(env, |app| {
            app.driver
                .app_mut()
                .engine
                .show_rich_text_popup(popup, anchor);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(())
        })
    }

    /// Closes the open rich-text popup (if any) without reporting a link
    /// click. Returns whether one was open.
    #[napi(js_name = "dismissRichTextPopup")]
    pub fn dismiss_rich_text_popup(&mut self, env: Env) -> Result<bool> {
        self.guarded(env, |app| {
            let closed = app.driver.app_mut().engine.dismiss_rich_text_popup();
            if closed {
                app.driver.render();
                take_pending_error(app.driver.app_mut())?;
            }
            Ok(closed)
        })
    }
}
