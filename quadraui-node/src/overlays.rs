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
//!
//! Every method also calls [`QuiApp::check_not_live`] first: it reaches
//! into `self.driver`'s own [`InteractionEngine`] directly, which is the
//! orphaned pre-live one once `startTui`/`startNative` has moved this
//! app's [`NodeAppLogic`] into the live runner, so without the guard a
//! call here would silently open the overlay on state no live
//! terminal/window ever renders.

use serde::Deserialize;

use super::*;

/// Which of [`QuiApp::show_overlay`]'s seven in-canvas overlay kinds a
/// `kind` string names. The 13 existing `show*`/`dismiss*` methods below
/// (`showContextMenu`, `showMessageDialog`/`dismissMessageDialog`,
/// `showToast`/`dismissToast`, `showTooltip`/`hideTooltip`,
/// `showCompletions`/`dismissCompletions`, `showFloat`/`dismissFloat`,
/// `showRichTextPopup`/`dismissRichTextPopup`) are one-line forwards onto
/// [`QuiApp::show_overlay`]/[`QuiApp::dismiss_overlay`] built on this enum
/// -- the napi-rs analog of `quadraui-py`'s own `OverlayKind`.
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

/// Shallow-copies `object`'s own enumerable properties into a fresh JS
/// object with `extra_key` set to `extra_value` (when `Some`) alongside
/// them. `showToast`'s own forward onto [`QuiApp::show_overlay`] uses this
/// to fold its `corner` argument into `toast`'s own shape without
/// mutating the object the caller passed in -- the napi-rs analog of
/// `quadraui-py`'s own `toast.copy()` before `set_item("corner", ...)`.
fn copy_with_field(
    env: Env,
    what: &str,
    object: Unknown,
    extra_key: &str,
    extra_value: Option<String>,
) -> Result<Unknown<'static>> {
    let source: Object = match object.get_type()? {
        // SAFETY: `get_type` just confirmed `object` is a JS object.
        ValueType::Object => unsafe { object.cast() }?,
        other => {
            return Err(invalid_descriptor_error(format!(
                "{what}: expected an object, got {other:?}"
            )))
        }
    };
    let mut copy = Object::new(&env)?;
    for key in Object::keys(&source)? {
        if let Some(value) = source.get::<Unknown>(&key)? {
            copy.set(key.as_str(), value)?;
        }
    }
    if let Some(extra_value) = extra_value {
        copy.set(extra_key, extra_value)?;
    }
    Ok(copy.to_unknown())
}

#[napi]
impl QuiApp {
    /// Opens one of this app's seven in-canvas overlay kinds by name -- the
    /// generic counterpart [`Self::show_context_menu`]/
    /// [`Self::show_message_dialog`]/[`Self::show_toast`]/
    /// [`Self::show_tooltip`]/[`Self::show_completions`]/
    /// [`Self::show_float`]/[`Self::show_rich_text_popup`] each forward
    /// onto (see each one's own doc for the exact user-facing shape and
    /// event-routing behavior its `kind` carries over unchanged).
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
    /// same object shape the matching `show*` method's own primitive
    /// documents (plus an extra `"corner"` key for `"toast"`, the named
    /// method's own scalar argument); for `"context_menu"`, a single
    /// `"items"` key wrapping that method's own list argument; for
    /// `"message_dialog"`, `"title"`/`"body"`/`"buttons"`/`"severity"` keys
    /// matching that method's own scalar arguments.
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
    /// Same "call from top-level script code only" restriction as
    /// [`Self::show_message_dialog`] -- every kind routes through this
    /// app's one `self.driver`, so a reentrant call from inside the
    /// registered event handler or render callback raises
    /// `ReentrantCallError` the same way.
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
                        decode_popover("showOverlay", &value, styles).map_err(decode_error_to_js)?
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
                        decode_layout_dialog("showOverlay", &value, styles)
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

    /// Closes the overlay named by `id` -- the generic counterpart
    /// [`Self::dismiss_message_dialog`]/[`Self::dismiss_toast`]/
    /// [`Self::hide_tooltip`]/[`Self::dismiss_completions`]/
    /// [`Self::dismiss_float`]/[`Self::dismiss_rich_text_popup`] each
    /// forward onto, plus a `"context_menu"` case none of those six had a
    /// dismiss counterpart for at all. Returns whether an overlay was
    /// actually open to close; no event is dispatched either way -- the
    /// same "close without resolving" shape every one of those six methods
    /// already documents.
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

    /// Opens a context menu built from `items` at `position` (an `{x, y}`
    /// object) -- [`InteractionEngine::open_context_menu`]. Selecting an
    /// item surfaces as `UiEvent::ContextMenuItemActivated`; dismissing
    /// with Escape or a click outside surfaces as
    /// `UiEvent::ContextMenuDismissed`, both through the normal registered
    /// handler. Calling this again while a popup from an earlier call is
    /// still open replaces it with the new one silently. A one-line
    /// forward onto [`Self::show_overlay`].
    #[napi(js_name = "showContextMenu")]
    pub fn show_context_menu(&mut self, env: Env, items: Unknown, position: Unknown) -> Result<()> {
        let mut descriptor = Object::new(&env)?;
        descriptor.set("items", items)?;
        self.show_overlay(
            env,
            "context_menu".to_string(),
            descriptor.to_unknown(),
            Some(position),
        )?;
        Ok(())
    }

    /// Opens an in-canvas confirm/alert/prompt dialog built from `title`/
    /// `body`/`buttons`/`severity` -- [`InteractionEngine::open_message_dialog`].
    /// `buttons` is a non-empty array of objects shaped like the in-canvas
    /// `Dialog` primitive's own `buttons` field. `severity` is one of
    /// `"info"`, `"question"`, `"warning"`, `"error"`, or `undefined`/`null`
    /// for no severity tint. Calling this again while a dialog from an
    /// earlier call is still open replaces it with the new one silently;
    /// call `dismissMessageDialog` to close without resolving. A one-line
    /// forward onto [`Self::show_overlay`].
    #[napi(js_name = "showMessageDialog")]
    pub fn show_message_dialog(
        &mut self,
        env: Env,
        title: String,
        body: String,
        buttons: Unknown,
        severity: Option<String>,
    ) -> Result<()> {
        let mut descriptor = Object::new(&env)?;
        descriptor.set("title", title)?;
        descriptor.set("body", body)?;
        descriptor.set("buttons", buttons)?;
        if let Some(severity) = severity {
            descriptor.set("severity", severity)?;
        }
        self.show_overlay(
            env,
            "message_dialog".to_string(),
            descriptor.to_unknown(),
            None,
        )?;
        Ok(())
    }

    /// Closes the open message dialog (if any) without resolving it --
    /// `UiEvent::MessageDialogResolved` does not fire. Returns whether a
    /// dialog was open to close. A one-line forward onto
    /// [`Self::dismiss_overlay`].
    #[napi(js_name = "dismissMessageDialog")]
    pub fn dismiss_message_dialog(&mut self, env: Env) -> Result<bool> {
        self.dismiss_overlay(env, "message_dialog".to_string())
    }

    /// Shows `toast` (an object matching [`Toast`]'s shape) stacked in
    /// `corner` (one of `"bottom_right"` (default), `"bottom_left"`,
    /// `"top_right"`, `"top_left"`) among whatever toasts are already
    /// showing -- [`InteractionEngine::show_toast`]. Replaces any existing
    /// toast with the same `id`. A click on an action button dispatches
    /// `UiEvent::ToastActionClicked(id)`; a click on the dismiss button
    /// closes it directly with no event. A one-line forward onto
    /// [`Self::show_overlay`].
    #[napi(js_name = "showToast")]
    pub fn show_toast(&mut self, env: Env, toast: Unknown, corner: Option<String>) -> Result<()> {
        let descriptor = copy_with_field(env, "showToast", toast, "corner", corner)?;
        self.show_overlay(env, "toast".to_string(), descriptor, None)?;
        Ok(())
    }

    /// Closes the toast with this id, if showing -- no event is dispatched.
    /// Returns whether one was closed. A one-line forward onto
    /// [`Self::dismiss_overlay`].
    #[napi(js_name = "dismissToast")]
    pub fn dismiss_toast(&mut self, env: Env, id: String) -> Result<bool> {
        self.dismiss_overlay(env, id)
    }

    /// Shows `tooltip` (an object matching [`quadraui::Tooltip`]'s shape)
    /// anchored against `anchor`, an `{x, y, width, height}` rect in this
    /// app's own cell grid -- [`InteractionEngine::show_tooltip`]. Replaces
    /// any tooltip already showing. The next `MouseDown`/`KeyPressed` this
    /// app handles auto-dismisses it; call `hideTooltip` to dismiss sooner.
    /// A one-line forward onto [`Self::show_overlay`].
    #[napi(js_name = "showTooltip")]
    pub fn show_tooltip(&mut self, env: Env, tooltip: Unknown, anchor: Unknown) -> Result<()> {
        self.show_overlay(env, "tooltip".to_string(), tooltip, Some(anchor))?;
        Ok(())
    }

    /// Hides the open tooltip (if any). Returns whether one was open. A
    /// one-line forward onto [`Self::dismiss_overlay`].
    #[napi(js_name = "hideTooltip")]
    pub fn hide_tooltip(&mut self, env: Env) -> Result<bool> {
        self.dismiss_overlay(env, "tooltip".to_string())
    }

    /// Shows `completions` (an object matching [`quadraui::Completions`]'s
    /// shape) anchored at `cursor`, an `{x, y}` caret position in this
    /// app's own cell grid -- [`InteractionEngine::show_completions`].
    /// Replaces any popup already open. A click on an item dispatches
    /// `UiEvent::CompletionItemActivated(id, index)` and closes the popup;
    /// Escape, or a click outside it, closes it with no event. A one-line
    /// forward onto [`Self::show_overlay`].
    #[napi(js_name = "showCompletions")]
    pub fn show_completions(
        &mut self,
        env: Env,
        completions: Unknown,
        cursor: Unknown,
    ) -> Result<()> {
        self.show_overlay(env, "completions".to_string(), completions, Some(cursor))?;
        Ok(())
    }

    /// Closes the open completions popup (if any) without reporting a
    /// selection. Returns whether one was open. A one-line forward onto
    /// [`Self::dismiss_overlay`].
    #[napi(js_name = "dismissCompletions")]
    pub fn dismiss_completions(&mut self, env: Env) -> Result<bool> {
        self.dismiss_overlay(env, "completions".to_string())
    }

    /// Opens `float` (an object matching [`quadraui::Float`]'s shape) at
    /// `width`x`height` cells -- [`InteractionEngine::show_float`]. Chrome
    /// only -- paints the float's background/border, nothing else; there
    /// is no content surface on this binding yet. Escape closes a
    /// focusable float (`float.focusable`, `true` by default) with no
    /// event; a non-focusable hint float never claims Escape. A one-line
    /// forward onto [`Self::show_overlay`].
    #[napi(js_name = "showFloat")]
    pub fn show_float(&mut self, env: Env, float: Unknown, width: f64, height: f64) -> Result<()> {
        let anchor = encode_via_json(env, "showFloat", &FloatSize { width, height })?;
        self.show_overlay(env, "float".to_string(), float, Some(anchor))?;
        Ok(())
    }

    /// Closes the open float (if any) with no event. Returns whether one
    /// was open. A one-line forward onto [`Self::dismiss_overlay`].
    #[napi(js_name = "dismissFloat")]
    pub fn dismiss_float(&mut self, env: Env) -> Result<bool> {
        self.dismiss_overlay(env, "float".to_string())
    }

    /// Shows `popup` (an object matching [`quadraui::RichTextPopup`]'s
    /// shape) anchored at `anchor`, an `{x, y}` position in this app's own
    /// cell grid -- [`InteractionEngine::show_rich_text_popup`]. Replaces
    /// any popup already open. A click on a link dispatches
    /// `UiEvent::RichTextLinkClicked(id, url)`; Escape, or a click outside
    /// the popup, closes it with no event. A one-line forward onto
    /// [`Self::show_overlay`].
    #[napi(js_name = "showRichTextPopup")]
    pub fn show_rich_text_popup(
        &mut self,
        env: Env,
        popup: Unknown,
        anchor: Unknown,
    ) -> Result<()> {
        self.show_overlay(env, "rich_text_popup".to_string(), popup, Some(anchor))?;
        Ok(())
    }

    /// Closes the open rich-text popup (if any) without reporting a link
    /// click. Returns whether one was open. A one-line forward onto
    /// [`Self::dismiss_overlay`].
    #[napi(js_name = "dismissRichTextPopup")]
    pub fn dismiss_rich_text_popup(&mut self, env: Env) -> Result<bool> {
        self.dismiss_overlay(env, "rich_text_popup".to_string())
    }
}

impl QuiApp {
    /// Shared body of every [`Self::show_overlay`] match arm: runs `open`
    /// against this app's live `(TuiBackend, NodeAppLogic)` pair, then
    /// re-renders so `screen()`/`screenContains()` reflect the newly
    /// opened overlay without a separate step -- the same immediate-render
    /// contract each of the 13 kind-specific `show*` methods already
    /// documents.
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
    /// re-rendering only when it reports something was actually closed --
    /// the same "no-op re-render" contract each of the six kind-specific
    /// `dismiss*`/`hideTooltip` methods already documents.
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
