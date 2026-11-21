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

use serde::Deserialize;

use super::*;

/// Which of [`QuiApp::show_overlay`]'s nine in-canvas overlay kinds a
/// `kind` string names -- see that method's own doc (and
/// [`QuiApp::dismiss_overlay`]'s) for the shared `descriptor`/`anchor` shape
/// each variant below expects, the napi-rs analog of `quadraui-py`'s own
/// `OverlayKind`.
#[derive(Clone, Copy)]
enum OverlayKind {
    ContextMenu,
    MessageDialog,
    Toast,
    Tooltip,
    Completions,
    Float,
    RichTextPopup,
    /// A float with a layout subtree painted inside it, whose leaves take
    /// focus and emit events exactly as in the main tree -- see
    /// [`InteractionEngine::show_popover_layout`].
    Popover,
    /// A modal dialog with a layout subtree painted in its body -- see
    /// [`InteractionEngine::open_layout_dialog`].
    LayoutDialog,
}

impl OverlayKind {
    /// Parses one of `showOverlay`'s seven recognized `kind` strings,
    /// prefixing an unrecognized one's error with `what` (the calling
    /// method's own name).
    fn parse(what: &str, kind: &str) -> Result<Self> {
        match kind {
            "context_menu" => Ok(Self::ContextMenu),
            "message_dialog" => Ok(Self::MessageDialog),
            "toast" => Ok(Self::Toast),
            "tooltip" => Ok(Self::Tooltip),
            "completions" => Ok(Self::Completions),
            "float" => Ok(Self::Float),
            "rich_text_popup" => Ok(Self::RichTextPopup),
            "popover" => Ok(Self::Popover),
            "layout_dialog" => Ok(Self::LayoutDialog),
            other => Err(invalid_descriptor_error(format!(
                "{what}: unknown overlay kind {other:?}; expected one of \"context_menu\", \
                 \"message_dialog\", \"toast\", \"tooltip\", \"completions\", \"float\", \
                 \"rich_text_popup\", \"popover\", \"layout_dialog\""
            ))),
        }
    }
}

/// `showOverlay`'s `"context_menu"` descriptor shape -- a single `"items"`
/// key wrapping `showContextMenu`'s own list argument.
#[derive(Deserialize)]
struct ContextMenuDescriptor {
    items: Vec<ContextMenuItem>,
}

/// `showOverlay`'s `"message_dialog"` descriptor shape --
/// `showMessageDialog`'s own `title`/`body`/`buttons`/`severity`
/// arguments as object keys.
#[derive(Deserialize)]
struct MessageDialogDescriptor {
    title: String,
    body: String,
    buttons: Vec<DialogButton>,
    severity: Option<String>,
}

/// `showOverlay`'s extra `"corner"` key on a `"toast"` descriptor --
/// `showToast`'s own scalar argument, read separately from the `Toast`
/// decode since [`Toast`] itself has no such field.
#[derive(Deserialize)]
struct ToastCornerField {
    corner: Option<String>,
}

/// `showOverlay`'s anchor shape for a `"float"` descriptor --
/// `showFloat`'s own `width`/`height` arguments folded into the shared
/// `anchor` parameter.
#[derive(Deserialize, Serialize)]
struct FloatSize {
    width: f64,
    height: f64,
}

/// Requires `anchor` to be `Some` for a `showOverlay` `kind` that needs one
/// (every kind but `"message_dialog"`/`"toast"`), raising
/// `InvalidDescriptorError` otherwise.
fn require_anchor<'env>(
    what: &str,
    kind: &str,
    anchor: Option<Unknown<'env>>,
) -> Result<Unknown<'env>> {
    anchor.ok_or_else(|| {
        invalid_descriptor_error(format!("{what}: {kind:?} overlay needs an anchor"))
    })
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
        let overlay_kind = OverlayKind::parse("showOverlay", &kind)?;
        match overlay_kind {
            OverlayKind::ContextMenu => {
                let parsed: ContextMenuDescriptor = decode_direct(env, "showOverlay", descriptor)?;
                let anchor = require_anchor("showOverlay", &kind, anchor)?;
                let position: Point = decode_direct(env, "showOverlay", anchor)?;
                self.open_overlay(env, |backend, app_logic| {
                    app_logic
                        .engine
                        .open_context_menu(parsed.items, position, backend);
                })?;
                Ok(kind)
            }
            OverlayKind::MessageDialog => {
                let parsed: MessageDialogDescriptor =
                    decode_direct(env, "showOverlay", descriptor)?;
                let severity = parsed
                    .severity
                    .map(|s| {
                        dialog_severity(&s)
                            .map_err(|err| invalid_descriptor_error(format!("showOverlay: {err}")))
                    })
                    .transpose()?;
                let opts = message_dialog_options_from_buttons(
                    parsed.title,
                    parsed.body,
                    parsed.buttons,
                    severity,
                )
                .map_err(|err| invalid_descriptor_error(format!("showOverlay: {err}")))?;
                self.open_overlay(env, |backend, app_logic| {
                    app_logic.engine.open_message_dialog(opts, backend);
                })?;
                Ok(kind)
            }
            OverlayKind::Toast => {
                let toast: Toast = decode_direct(env, "showOverlay", descriptor)?;
                let corner: ToastCornerField = decode_direct(env, "showOverlay", descriptor)?;
                let corner = corner
                    .corner
                    .map(|c| {
                        toast_corner(&c)
                            .map_err(|err| invalid_descriptor_error(format!("showOverlay: {err}")))
                    })
                    .transpose()?
                    .unwrap_or_default();
                let id = toast.id.to_string();
                self.open_overlay(env, |_backend, app_logic| {
                    app_logic.engine.show_toast(toast, corner);
                })?;
                Ok(id)
            }
            OverlayKind::Tooltip => {
                let tooltip: quadraui::Tooltip = decode_direct(env, "showOverlay", descriptor)?;
                let anchor = require_anchor("showOverlay", &kind, anchor)?;
                let anchor: Rect = decode_direct(env, "showOverlay", anchor)?;
                let id = tooltip.id.to_string();
                self.open_overlay(env, |_backend, app_logic| {
                    app_logic.engine.show_tooltip(tooltip, anchor);
                })?;
                Ok(id)
            }
            OverlayKind::Completions => {
                let completions: quadraui::Completions =
                    decode_direct(env, "showOverlay", descriptor)?;
                let anchor = require_anchor("showOverlay", &kind, anchor)?;
                let cursor: Point = decode_direct(env, "showOverlay", anchor)?;
                let id = completions.id.to_string();
                self.open_overlay(env, |_backend, app_logic| {
                    app_logic.engine.show_completions(completions, cursor);
                })?;
                Ok(id)
            }
            OverlayKind::Float => {
                let float: quadraui::Float = decode_direct(env, "showOverlay", descriptor)?;
                let anchor = require_anchor("showOverlay", &kind, anchor)?;
                let size: FloatSize = decode_direct(env, "showOverlay", anchor)?;
                let measure = FloatMeasure::new(size.width as f32, size.height as f32);
                let id = float.id.to_string();
                self.open_overlay(env, |backend, app_logic| {
                    app_logic.engine.show_float(float, measure, backend);
                })?;
                Ok(id)
            }
            OverlayKind::RichTextPopup => {
                let popup: quadraui::RichTextPopup = decode_direct(env, "showOverlay", descriptor)?;
                let anchor = require_anchor("showOverlay", &kind, anchor)?;
                let anchor: Point = decode_direct(env, "showOverlay", anchor)?;
                let id = popup.id.to_string();
                self.open_overlay(env, |_backend, app_logic| {
                    app_logic.engine.show_rich_text_popup(popup, anchor);
                })?;
                Ok(id)
            }
            OverlayKind::Popover => {
                let anchor = require_anchor("showOverlay", &kind, anchor)?;
                let size: FloatSize = decode_direct(env, "showOverlay", anchor)?;
                let measure = FloatMeasure::new(size.width as f32, size.height as f32);
                let value = to_descriptor_value(descriptor)
                    .map_err(|detail| invalid_descriptor_error(format!("showOverlay: {detail}")))?;
                let mut id = String::new();
                self.open_overlay_try(env, |backend, app_logic| {
                    let decoded = {
                        let theme = backend.theme();
                        let styles = StyleContext::new(&app_logic.host.style_sets, &theme);
                        decode_popover(
                            "showOverlay",
                            &value,
                            styles,
                            &app_logic.host.wire_diagnostics,
                        )
                        .map_err(decode_error_to_js)?
                    };
                    id = decoded.chrome.id.to_string();
                    app_logic.engine.show_popover_layout(
                        decoded.chrome,
                        measure,
                        decoded.content,
                        backend,
                    );
                    Ok(())
                })?;
                Ok(id)
            }
            OverlayKind::LayoutDialog => {
                // Unlike every other anchored kind, the anchor is
                // optional here: it sizes the dialog's content slot, and
                // omitting it takes `DialogContentSize`'s own
                // viewport-relative defaults.
                let size = match anchor {
                    Some(anchor) => {
                        let size: FloatSize = decode_direct(env, "showOverlay", anchor)?;
                        DialogContentSize {
                            width: Some(size.width as f32),
                            height: Some(size.height as f32),
                        }
                    }
                    None => DialogContentSize::default(),
                };
                let value = to_descriptor_value(descriptor)
                    .map_err(|detail| invalid_descriptor_error(format!("showOverlay: {detail}")))?;
                let mut id = String::new();
                self.open_overlay_try(env, |backend, app_logic| {
                    let decoded = {
                        let theme = backend.theme();
                        let styles = StyleContext::new(&app_logic.host.style_sets, &theme);
                        decode_layout_dialog(
                            "showOverlay",
                            &value,
                            styles,
                            &app_logic.host.wire_diagnostics,
                        )
                        .map_err(decode_error_to_js)?
                    };
                    id = decoded.chrome.id.to_string();
                    app_logic.engine.open_layout_dialog(
                        decoded.chrome,
                        decoded.content,
                        size,
                        backend,
                    );
                    Ok(())
                })?;
                Ok(id)
            }
        }
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

/// Reads `value[chrome_key]["id"]` out of a `showOverlay` descriptor
/// already converted to JSON, without running the rest of
/// [`decode_popover`]/[`decode_layout_dialog`]'s own validation --
/// [`QuiAppActions::show_overlay`]'s only way to return a `"popover"`/
/// `"layout_dialog"` overlay's id synchronously, since the full decode
/// needs a live backend's theme that only exists once
/// [`flush_deferred_actions`] runs (see [`DeferredAction::Popover`]'s own
/// doc).
fn peek_overlay_content_id(what: &str, value: &JsonValue, chrome_key: &str) -> Result<String> {
    value
        .get(chrome_key)
        .and_then(|chrome| chrome.get("id"))
        .and_then(|id| id.as_str())
        .map(|id| id.to_string())
        .ok_or_else(|| {
            invalid_descriptor_error(format!("{what}: missing {chrome_key:?}.\"id\" string"))
        })
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
        env: Env,
        kind: String,
        descriptor: Unknown,
        anchor: Option<Unknown>,
    ) -> Result<String> {
        let overlay_kind = OverlayKind::parse("showOverlay", &kind)?;
        match overlay_kind {
            OverlayKind::ContextMenu => {
                let parsed: ContextMenuDescriptor = decode_direct(env, "showOverlay", descriptor)?;
                let anchor = require_anchor("showOverlay", &kind, anchor)?;
                let position: Point = decode_direct(env, "showOverlay", anchor)?;
                self.queue
                    .borrow_mut()
                    .push_back(DeferredAction::ContextMenu(parsed.items, position));
                Ok(kind)
            }
            OverlayKind::MessageDialog => {
                let parsed: MessageDialogDescriptor =
                    decode_direct(env, "showOverlay", descriptor)?;
                let severity = parsed
                    .severity
                    .map(|s| {
                        dialog_severity(&s)
                            .map_err(|err| invalid_descriptor_error(format!("showOverlay: {err}")))
                    })
                    .transpose()?;
                let opts = message_dialog_options_from_buttons(
                    parsed.title,
                    parsed.body,
                    parsed.buttons,
                    severity,
                )
                .map_err(|err| invalid_descriptor_error(format!("showOverlay: {err}")))?;
                self.queue
                    .borrow_mut()
                    .push_back(DeferredAction::MessageDialog(opts));
                Ok(kind)
            }
            OverlayKind::Toast => {
                let toast: Toast = decode_direct(env, "showOverlay", descriptor)?;
                let corner: ToastCornerField = decode_direct(env, "showOverlay", descriptor)?;
                let corner = corner
                    .corner
                    .map(|c| {
                        toast_corner(&c)
                            .map_err(|err| invalid_descriptor_error(format!("showOverlay: {err}")))
                    })
                    .transpose()?
                    .unwrap_or_default();
                let id = toast.id.to_string();
                self.queue
                    .borrow_mut()
                    .push_back(DeferredAction::Toast(toast, corner));
                Ok(id)
            }
            OverlayKind::Tooltip => {
                let tooltip: quadraui::Tooltip = decode_direct(env, "showOverlay", descriptor)?;
                let anchor = require_anchor("showOverlay", &kind, anchor)?;
                let anchor: Rect = decode_direct(env, "showOverlay", anchor)?;
                let id = tooltip.id.to_string();
                self.queue
                    .borrow_mut()
                    .push_back(DeferredAction::Tooltip(tooltip, anchor));
                Ok(id)
            }
            OverlayKind::Completions => {
                let completions: quadraui::Completions =
                    decode_direct(env, "showOverlay", descriptor)?;
                let anchor = require_anchor("showOverlay", &kind, anchor)?;
                let cursor: Point = decode_direct(env, "showOverlay", anchor)?;
                let id = completions.id.to_string();
                self.queue
                    .borrow_mut()
                    .push_back(DeferredAction::Completions(completions, cursor));
                Ok(id)
            }
            OverlayKind::Float => {
                let float: quadraui::Float = decode_direct(env, "showOverlay", descriptor)?;
                let anchor = require_anchor("showOverlay", &kind, anchor)?;
                let size: FloatSize = decode_direct(env, "showOverlay", anchor)?;
                let measure = FloatMeasure::new(size.width as f32, size.height as f32);
                let id = float.id.to_string();
                self.queue
                    .borrow_mut()
                    .push_back(DeferredAction::Float(float, measure));
                Ok(id)
            }
            OverlayKind::RichTextPopup => {
                let popup: quadraui::RichTextPopup = decode_direct(env, "showOverlay", descriptor)?;
                let anchor = require_anchor("showOverlay", &kind, anchor)?;
                let anchor: Point = decode_direct(env, "showOverlay", anchor)?;
                let id = popup.id.to_string();
                self.queue
                    .borrow_mut()
                    .push_back(DeferredAction::RichTextPopup(popup, anchor));
                Ok(id)
            }
            OverlayKind::Popover => {
                let anchor = require_anchor("showOverlay", &kind, anchor)?;
                let size: FloatSize = decode_direct(env, "showOverlay", anchor)?;
                let measure = FloatMeasure::new(size.width as f32, size.height as f32);
                let value = to_descriptor_value(descriptor)
                    .map_err(|detail| invalid_descriptor_error(format!("showOverlay: {detail}")))?;
                let id = peek_overlay_content_id("showOverlay", &value, "float")?;
                self.queue
                    .borrow_mut()
                    .push_back(DeferredAction::Popover(value, measure));
                Ok(id)
            }
            OverlayKind::LayoutDialog => {
                // Unlike every other anchored kind, the anchor is
                // optional here: it sizes the dialog's content slot, and
                // omitting it takes `DialogContentSize`'s own
                // viewport-relative defaults.
                let size = match anchor {
                    Some(anchor) => {
                        let size: FloatSize = decode_direct(env, "showOverlay", anchor)?;
                        DialogContentSize {
                            width: Some(size.width as f32),
                            height: Some(size.height as f32),
                        }
                    }
                    None => DialogContentSize::default(),
                };
                let value = to_descriptor_value(descriptor)
                    .map_err(|detail| invalid_descriptor_error(format!("showOverlay: {detail}")))?;
                let id = peek_overlay_content_id("showOverlay", &value, "dialog")?;
                self.queue
                    .borrow_mut()
                    .push_back(DeferredAction::LayoutDialog(value, size));
                Ok(id)
            }
        }
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
    /// Shared body of every [`Self::show_overlay`] match arm: runs `open`
    /// against this app's live `(TuiBackend, NodeAppLogic)` pair, then
    /// re-renders so `screen()`/`screenContains()` reflect the newly
    /// opened overlay without a separate step.
    fn open_overlay(
        &mut self,
        env: Env,
        open: impl FnOnce(&mut dyn Backend, &mut NodeAppLogic),
    ) -> Result<()> {
        self.guarded(env, |app| {
            let (backend, app_logic) = app.driver.parts_mut();
            open(backend, app_logic);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(())
        })
    }

    /// [`Self::open_overlay`] for a kind whose descriptor can only be
    /// decoded once the live `(TuiBackend, NodeAppLogic)` pair is in hand
    /// -- `"popover"`/`"layout_dialog"`, whose `"content"` subtree
    /// resolves its `"class"` keys against the backend's current theme
    /// and this app's own registered style sets. `open` reports a decode
    /// failure as the same `InvalidDescriptorError` every other kind
    /// raises before ever reaching the engine, and nothing is rendered
    /// when it does.
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
