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
//! reentrant call here raises `ReentrantCallError` the same way every
//! other `QuiAppActions`-backed method does (see the module doc's "cannot
//! call back into `QuiApp`" note). From inside a handler, call the matching
//! [`QuiAppActions`] method (via `QuiApp::actions()`) instead.
//!
//! Every method also calls [`QuiApp::check_not_live`] first: it reaches
//! into `self.driver`'s own [`InteractionEngine`] directly, which is the
//! orphaned pre-live one once `startTui`/`startNative` has moved this
//! app's [`NodeAppLogic`] into the live runner, so without the guard a
//! call here would silently open the overlay on state no live
//! terminal/window ever renders.

use super::*;

/// Decodes one `showOverlay(kind, descriptor, anchor)` call through
/// [`quadraui_binding_core::decode_overlay`] into the action that opens the
/// overlay and the id it is dismissed by; a malformed call raises
/// `InvalidDescriptorError`. `anchor` is an object whose shape `kind`
/// selects (see [`overlay_anchor_from_value`]).
fn decode_show_overlay(
    kind: &str,
    descriptor: Unknown,
    anchor: Option<Unknown>,
) -> Result<(DeferredAction, String)> {
    let value = to_descriptor_value(descriptor)
        .map_err(|detail| invalid_descriptor_error(format!("showOverlay: {detail}")))?;
    let anchor = match anchor {
        Some(anchor) => {
            let anchor = to_descriptor_value(anchor)
                .map_err(|detail| invalid_descriptor_error(format!("showOverlay: {detail}")))?;
            if anchor.is_null() {
                None
            } else {
                Some(
                    overlay_anchor_from_value("showOverlay", kind, &anchor)
                        .map_err(invalid_descriptor_error)?,
                )
            }
        }
        None => None,
    };
    decode_overlay("showOverlay", kind, &value, anchor).map_err(invalid_descriptor_error)
}

#[napi]
impl QuiApp {
    /// Opens one of this app's nine in-canvas overlay kinds by name.
    ///
    /// `kind` is one of `"context_menu"`, `"message_dialog"`, `"toast"`,
    /// `"tooltip"`, `"completions"`, `"float"`, `"rich_text_popup"`,
    /// `"popover"`, `"layout_dialog"` -- `InvalidDescriptorError` names
    /// the full list for anything else.
    ///
    /// `"popover"` and `"layout_dialog"` are the content-bearing kinds:
    /// each takes a `{float|dialog: {...}, content: <layout node>}`
    /// descriptor (`PopoverDescriptor`/`LayoutDialogDescriptor` in
    /// `wire.d.ts`) whose `content` subtree is laid out inside the
    /// overlay and whose leaves take focus and emit the same semantic
    /// events they would in the main tree. `anchor` is a `{width,
    /// height}` size for both -- the popover's own size, or the dialog's
    /// content slot, which may be omitted entirely for a
    /// viewport-relative default.
    /// `descriptor` holds that kind's own fields: for `"toast"`,
    /// `"tooltip"`, `"completions"`, `"float"`, `"rich_text_popup"`, the
    /// matching `quadraui` primitive's own shape (plus an extra `"corner"`
    /// key for `"toast"`, the stacking corner); for `"context_menu"`, a
    /// single `"items"` key wrapping a list of `ContextMenuItem` objects;
    /// for `"message_dialog"`, `"title"`/`"body"`/`"buttons"`/`"severity"`
    /// keys.
    ///
    /// `anchor` reshapes each kind's own positional argument into one
    /// object: an `{x, y}` point for `"context_menu"`/`"completions"`/
    /// `"rich_text_popup"`, an `{x, y, width, height}` rect for
    /// `"tooltip"`, a `{width, height}` size for `"float"`, and
    /// `"message_dialog"`/`"toast"` ignore it entirely (`undefined`/`null`
    /// is fine for both; every other kind raises `InvalidDescriptorError`
    /// if `anchor` is absent).
    ///
    /// Returns an id this overlay can later be named by in
    /// [`Self::dismiss_overlay`]: the descriptor's own `"id"` field for
    /// every kind that has one (`"toast"`, `"tooltip"`, `"completions"`,
    /// `"float"`, `"rich_text_popup"`), or a fixed sentinel -- `kind`
    /// itself -- for the two that don't (`"context_menu"`,
    /// `"message_dialog"`).
    ///
    /// Call from top-level script code only -- every kind routes through
    /// this app's one `self.driver`, so a reentrant call from inside the
    /// registered event handler or render callback raises
    /// `ReentrantCallError` the same way every other restricted `QuiApp`
    /// method does; from inside a handler, call [`QuiAppActions::show_overlay`]
    /// (via `QuiApp::actions()`) instead.
    #[napi(js_name = "showOverlay")]
    pub fn show_overlay(
        &mut self,
        env: Env,
        kind: String,
        descriptor: Unknown,
        anchor: Option<Unknown>,
    ) -> Result<String> {
        self.check_not_dispatching()?;
        self.check_not_live()?;
        let (action, id) = decode_show_overlay(&kind, descriptor, anchor)?;
        self.open_overlay_try(env, |backend, app_logic| {
            apply_deferred_action(
                action,
                backend,
                &mut app_logic.engine,
                &mut app_logic.host,
                "showOverlay",
            )
            .map_err(deferred_error_to_napi)
        })?;
        Ok(id)
    }

    /// Closes the overlay named by `id`. Returns whether an overlay was
    /// actually open to close; no event is dispatched either way.
    ///
    /// `id` is either one of `showOverlay`'s own six singleton sentinels
    /// (`"context_menu"`, `"message_dialog"`, `"tooltip"`, `"completions"`,
    /// `"float"`, `"rich_text_popup"` -- closing the matching kind's open
    /// overlay regardless of whether it's the one that returned this exact
    /// id) or any other string, read as a toast id (closing the toast with
    /// that id, if one is showing). A toast whose own id happens to
    /// collide with one of those six sentinels is the one case this can't
    /// address by id; name toasts something else.
    #[napi(js_name = "dismissOverlay")]
    pub fn dismiss_overlay(&mut self, env: Env, id: String) -> Result<bool> {
        self.check_not_live()?;
        match id.as_str() {
            "context_menu" => self.dismiss_via(env, |backend, app_logic| {
                app_logic.engine.dismiss_context_menu(backend)
            }),
            "message_dialog" => self.dismiss_via(env, |backend, app_logic| {
                app_logic.engine.close_message_dialog(backend)
            }),
            "tooltip" => {
                self.dismiss_via(env, |_backend, app_logic| app_logic.engine.hide_tooltip())
            }
            "completions" => self.dismiss_via(env, |_backend, app_logic| {
                app_logic.engine.dismiss_completions()
            }),
            // A `"popover"` *is* a float (one this engine paints content
            // into), so either sentinel closes it.
            "float" | "popover" => self.dismiss_via(env, |backend, app_logic| {
                app_logic.engine.dismiss_float(backend)
            }),
            "layout_dialog" => self.dismiss_via(env, |backend, app_logic| {
                app_logic.engine.close_layout_dialog(backend)
            }),
            "rich_text_popup" => self.dismiss_via(env, |_backend, app_logic| {
                app_logic.engine.dismiss_rich_text_popup()
            }),
            toast_id => {
                let toast_id = WidgetId::new(toast_id);
                self.dismiss_via(env, move |_backend, app_logic| {
                    app_logic.engine.dismiss_toast(toast_id)
                })
            }
        }
    }
}

#[napi]
impl QuiAppActions {
    /// Queues one of [`DeferredAction::ContextMenu`]/
    /// [`DeferredAction::MessageDialog`]/[`DeferredAction::Toast`]/
    /// [`DeferredAction::Tooltip`]/[`DeferredAction::Completions`]/
    /// [`DeferredAction::Float`]/[`DeferredAction::RichTextPopup`]/
    /// [`DeferredAction::Popover`]/[`DeferredAction::LayoutDialog`],
    /// depending on `kind` -- same `kind` vocabulary, same `descriptor`/
    /// `anchor` shapes, as [`QuiApp::show_overlay`]; see that method's own
    /// doc for the full per-kind contract.
    ///
    /// Returns the same id [`QuiApp::show_overlay`] would return. For
    /// `"popover"`/`"layout_dialog"`, that id is read straight off
    /// `descriptor`'s own `"float"`/`"dialog"` chrome key -- the rest of
    /// the decode [`QuiApp::show_overlay`] runs eagerly instead waits for
    /// `flush_deferred_actions`, the only place a live backend's theme
    /// (needed to resolve the content subtree's own `"class"` keys)
    /// exists -- see [`DeferredAction::Popover`]'s own doc.
    #[napi(js_name = "showOverlay")]
    pub fn show_overlay(
        &self,
        _env: Env,
        kind: String,
        descriptor: Unknown,
        anchor: Option<Unknown>,
    ) -> Result<String> {
        let (action, id) = decode_show_overlay(&kind, descriptor, anchor)?;
        self.queue.borrow_mut().push_back(action);
        Ok(id)
    }

    /// Queues a [`DeferredAction::DismissOverlay`] -- same `id` vocabulary
    /// as [`QuiApp::dismiss_overlay`]. Unlike that method, there is
    /// nothing open yet to report closing -- `flush_deferred_actions`
    /// applies this only after the current dispatch returns -- so this
    /// returns nothing rather than whether something was open to close.
    #[napi(js_name = "dismissOverlay")]
    pub fn dismiss_overlay(&self, id: String) -> Result<()> {
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::DismissOverlay(id));
        Ok(())
    }
}

impl QuiApp {
    /// Shared body of `show_overlay`: runs `open` against this app's live
    /// `(TuiBackend, NodeAppLogic)` pair, then re-renders so the screen reflects the newly
    /// opened overlay without a separate step. A failure `open` reports
    /// (a popover or layout-dialog content subtree that does not decode)
    /// raises, and nothing is rendered.
    fn open_overlay_try(
        &mut self,
        env: Env,
        open: impl FnOnce(&mut dyn Backend, &mut NodeAppLogic) -> Result<()>,
    ) -> Result<()> {
        self.guarded(env, |app| {
            let (backend, app_logic) = app.driver.parts_mut();
            open(backend, app_logic)?;
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(())
        })
    }

    /// Shared body of every [`Self::dismiss_overlay`] match arm: runs
    /// `close` against this app's live `(TuiBackend, NodeAppLogic)` pair,
    /// re-rendering only when it reports something was actually closed.
    fn dismiss_via(
        &mut self,
        env: Env,
        close: impl FnOnce(&mut dyn Backend, &mut NodeAppLogic) -> bool,
    ) -> Result<bool> {
        self.guarded(env, |app| {
            let (backend, app_logic) = app.driver.parts_mut();
            let closed = close(backend, app_logic);
            if closed {
                app.driver.render();
                take_pending_error(app.driver.app_mut())?;
            }
            Ok(closed)
        })
    }
}
