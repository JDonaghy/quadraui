//! Host calls a binding queues from inside an event handler and applies
//! once the handler has returned.
//!
//! A binding's handler runs while the engine and backend are borrowed for
//! dispatch, so an overlay, window, tray or platform-service call made
//! from inside it cannot run immediately. The binding pushes a
//! [`DeferredAction`] instead and, after dispatch, hands each one to
//! [`apply_deferred_action`] with the live backend. The binding only maps
//! a [`DeferredError`] onto its own exception type; the action vocabulary
//! and what each action does live here, once, for every binding.

use std::path::PathBuf;

use serde::Deserialize;
use std::time::Duration;

use quadraui::{
    Backend, Completions, ContextMenuItem, Float, FloatMeasure, ImageSource, MessageDialogOptions,
    Notification, Point, PointerShape, Rect, RichTextPopup, Theme, Toast, ToastCorner, Tooltip,
    TreeTableNodeData, WidgetId,
};

use crate::engine::{DialogContentSize, InteractionEngine};
use crate::host::{apply_tab_activated, menu_for_surface, with_tray, with_window, BindingHost};
use crate::layout::StyleOverride;
use crate::services::{
    add_recent_document, begin_drag_out, clear_recent_documents, end_power_save_block,
    remove_as_default_protocol_client, set_as_default_protocol_client, set_dock_menu,
    set_jump_list, set_login_item, update_apply_on_quit, ServiceError,
};
use crate::wire::{decode_layout_dialog, decode_popover, StyleContext};
use crate::DecodeError;

/// One queued host call. Each variant carries exactly the arguments the
/// matching immediate call takes, already validated and converted at
/// queue time, except [`Self::Popover`]/[`Self::LayoutDialog`]: decoding
/// their `"content"` subtree needs the live backend's theme, so they keep
/// the raw descriptor and [`apply_deferred_action`] decodes it.
pub enum DeferredAction {
    MessageDialog(MessageDialogOptions),
    ContextMenu(Vec<ContextMenuItem>, Point),
    Toast(Toast, ToastCorner),
    Tooltip(Tooltip, Rect),
    Completions(Completions, Point),
    Float(Float, FloatMeasure),
    RichTextPopup(RichTextPopup, Point),
    Popover(serde_json::Value, FloatMeasure),
    LayoutDialog(serde_json::Value, DialogContentSize),
    /// One of the overlay sentinels (`"context_menu"`, `"message_dialog"`,
    /// `"tooltip"`, `"completions"`, `"float"`/`"popover"`,
    /// `"layout_dialog"`, `"rich_text_popup"`) or a toast id.
    DismissOverlay(String),
    Notification(Notification),
    OpenPath(PathBuf),
    RevealInFileManager(PathBuf),
    MoveToTrash(PathBuf),
    RestoreFromTrash(PathBuf),
    Title(String),
    DocumentEdited(bool),
    RepresentedFilename(Option<PathBuf>),
    TrayIcon(ImageSource),
    TrayTooltip(String),
    TrayMenu(Vec<ContextMenuItem>),
    Cursor(PointerShape),
    /// `(field, value)` pairs already validated against `Theme`'s fields,
    /// merged over the backend's current theme.
    Theme(Vec<(String, serde_json::Value)>),
    ApplyUpdateOnQuit(u64),
    EndPowerSaveBlock(u64),
    SetLoginItem {
        enabled: bool,
        open_hidden: bool,
        args: Vec<String>,
    },
    SetAsDefaultProtocolClient(String),
    RemoveAsDefaultProtocolClient(String),
    BeginDragOut {
        files: Vec<String>,
        text: Option<String>,
    },
    AddRecentDocument(PathBuf),
    ClearRecentDocuments,
    SetDockMenu(serde_json::Value),
    SetJumpList(serde_json::Value),
    ActivateTab(WidgetId, usize),
    // Boxed: `StyleOverride` covers every `Theme`/`Style` token, so storing
    // it inline would dominate the enum's size.
    DefineStyle(String, Box<StyleOverride>),
    SetTreeTableChildren(WidgetId, String, Vec<TreeTableNodeData>),
    SetTreeTableSortOverride(WidgetId, Vec<TreeTableNodeData>),
}

/// The `"context_menu"` `show_overlay` descriptor: the menu's items.
#[derive(serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ContextMenuOverlayDescriptor {
    pub items: Vec<ContextMenuItem>,
}

/// The `"message_dialog"` `show_overlay` descriptor. `severity` is
/// `"info"`, `"question"`, `"warning"` or `"error"`.
#[derive(serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MessageDialogOverlayDescriptor {
    pub title: String,
    pub body: String,
    pub buttons: Vec<quadraui::DialogButton>,
    #[serde(default)]
    pub severity: Option<String>,
}

/// The `"corner"` key a `"toast"` `show_overlay` descriptor carries beside
/// the [`Toast`] fields: `"bottom_right"`, `"bottom_left"`, `"top_right"`
/// or `"top_left"`.
#[derive(serde::Deserialize)]
struct ToastCornerField {
    #[serde(default)]
    corner: Option<String>,
}

/// Every `kind` [`decode_overlay`] accepts.
pub const OVERLAY_KINDS: [&str; 9] = [
    "context_menu",
    "message_dialog",
    "toast",
    "tooltip",
    "completions",
    "float",
    "rich_text_popup",
    "popover",
    "layout_dialog",
];

/// Decodes one `show_overlay(kind, descriptor, anchor)` call into the
/// [`DeferredAction`] that opens it, plus the id the overlay is dismissed
/// by: the descriptor's own id, the popover/layout-dialog chrome id, or
/// `kind` itself for `"context_menu"`/`"message_dialog"`.
///
/// `anchor` is positional: `[x, y, _, _]` for `"context_menu"`,
/// `"completions"` and `"rich_text_popup"`; `[x, y, width, height]` for
/// `"tooltip"`; `[width, height, _, _]` for `"float"`, `"popover"` and
/// `"layout_dialog"` (optional for the last). `"message_dialog"` and
/// `"toast"` ignore it. A binding whose anchor is an object converts it
/// with [`overlay_anchor_from_value`]. Errors carry `what: ` as prefix.
pub fn decode_overlay(
    what: &str,
    kind: &str,
    descriptor: &serde_json::Value,
    anchor: Option<[f32; 4]>,
) -> Result<(DeferredAction, String), String> {
    fn parse<T: serde::de::DeserializeOwned>(
        what: &str,
        value: &serde_json::Value,
    ) -> Result<T, String> {
        T::deserialize(value).map_err(|err| format!("{what}: {err}"))
    }
    let need_anchor = || anchor.ok_or_else(|| format!("{what}: {kind:?} overlay needs an anchor"));
    let peek_chrome_id = |chrome_key: &str| {
        descriptor
            .get(chrome_key)
            .and_then(|chrome| chrome.get("id"))
            .and_then(|id| id.as_str())
            .map(str::to_string)
            .ok_or_else(|| format!("{what}: missing {chrome_key:?}.\"id\" string"))
    };
    match kind {
        "context_menu" => {
            let parsed: ContextMenuOverlayDescriptor = parse(what, descriptor)?;
            let [x, y, ..] = need_anchor()?;
            Ok((
                DeferredAction::ContextMenu(parsed.items, Point::new(x, y)),
                kind.to_string(),
            ))
        }
        "message_dialog" => {
            let parsed: MessageDialogOverlayDescriptor = parse(what, descriptor)?;
            let severity = parsed
                .severity
                .map(|s| crate::dialog_severity(&s).map_err(|err| format!("{what}: {err}")))
                .transpose()?;
            let opts = crate::message_dialog_options_from_buttons(
                parsed.title,
                parsed.body,
                parsed.buttons,
                severity,
            )
            .map_err(|err| format!("{what}: {err}"))?;
            Ok((DeferredAction::MessageDialog(opts), kind.to_string()))
        }
        "toast" => {
            let toast: Toast = parse(what, descriptor)?;
            let corner: ToastCornerField = parse(what, descriptor)?;
            let corner = corner
                .corner
                .map(|c| crate::toast_corner(&c).map_err(|err| format!("{what}: {err}")))
                .transpose()?
                .unwrap_or_default();
            let id = toast.id.to_string();
            Ok((DeferredAction::Toast(toast, corner), id))
        }
        "tooltip" => {
            let tooltip: Tooltip = parse(what, descriptor)?;
            let [x, y, width, height] = need_anchor()?;
            let id = tooltip.id.to_string();
            Ok((
                DeferredAction::Tooltip(tooltip, Rect::new(x, y, width, height)),
                id,
            ))
        }
        "completions" => {
            let completions: Completions = parse(what, descriptor)?;
            let [x, y, ..] = need_anchor()?;
            let id = completions.id.to_string();
            Ok((
                DeferredAction::Completions(completions, Point::new(x, y)),
                id,
            ))
        }
        "float" => {
            let float: Float = parse(what, descriptor)?;
            let [width, height, ..] = need_anchor()?;
            let id = float.id.to_string();
            Ok((
                DeferredAction::Float(float, FloatMeasure::new(width, height)),
                id,
            ))
        }
        "rich_text_popup" => {
            let popup: RichTextPopup = parse(what, descriptor)?;
            let [x, y, ..] = need_anchor()?;
            let id = popup.id.to_string();
            Ok((DeferredAction::RichTextPopup(popup, Point::new(x, y)), id))
        }
        "popover" => {
            let [width, height, ..] = need_anchor()?;
            let id = peek_chrome_id("float")?;
            Ok((
                DeferredAction::Popover(descriptor.clone(), FloatMeasure::new(width, height)),
                id,
            ))
        }
        "layout_dialog" => {
            let size = match anchor {
                Some([width, height, ..]) => DialogContentSize {
                    width: Some(width),
                    height: Some(height),
                },
                None => DialogContentSize::default(),
            };
            let id = peek_chrome_id("dialog")?;
            Ok((DeferredAction::LayoutDialog(descriptor.clone(), size), id))
        }
        other => Err(format!(
            "{what}: unknown overlay kind {other:?}; expected one of {}",
            OVERLAY_KINDS
                .iter()
                .map(|kind| format!("{kind:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Converts an object-shaped `show_overlay` anchor into [`decode_overlay`]'s
/// positional one, reading the shape `kind` expects: `{x, y}` for
/// `"context_menu"`/`"completions"`/`"rich_text_popup"`, `{x, y, width,
/// height}` for `"tooltip"`, `{width, height}` for `"float"`/`"popover"`/
/// `"layout_dialog"`. `"message_dialog"`, `"toast"` and an unknown kind read
/// nothing and yield `[0.0; 4]`; [`decode_overlay`] reports the unknown kind.
pub fn overlay_anchor_from_value(
    what: &str,
    kind: &str,
    anchor: &serde_json::Value,
) -> Result<[f32; 4], String> {
    #[derive(serde::Deserialize)]
    struct Size {
        width: f32,
        height: f32,
    }
    let err = |err: serde_json::Error| format!("{what}: {err}");
    match kind {
        "context_menu" | "completions" | "rich_text_popup" => {
            let point = Point::deserialize(anchor).map_err(err)?;
            Ok([point.x, point.y, 0.0, 0.0])
        }
        "tooltip" => {
            let rect = Rect::deserialize(anchor).map_err(err)?;
            Ok([rect.x, rect.y, rect.width, rect.height])
        }
        "float" | "popover" | "layout_dialog" => {
            let size = Size::deserialize(anchor).map_err(err)?;
            Ok([size.width, size.height, 0.0, 0.0])
        }
        _ => Ok([0.0; 4]),
    }
}

/// Why [`apply_deferred_action`] could not complete an action. `op` names
/// the host call, for the binding's error message.
#[derive(Debug)]
pub enum DeferredError {
    /// A popover or layout-dialog descriptor failed to decode.
    Decode(DecodeError),
    /// A backend window, tray or platform-service call failed.
    Backend {
        op: &'static str,
        error: quadraui::BackendError,
    },
    /// A [`crate::services`] helper failed.
    Service {
        op: &'static str,
        error: ServiceError,
    },
}

fn backend_call(
    op: &'static str,
    result: quadraui::ServiceResult<()>,
) -> Result<(), DeferredError> {
    result.map_err(|error| DeferredError::Backend { op, error })
}

fn service_call(op: &'static str, result: Result<(), ServiceError>) -> Result<(), DeferredError> {
    result.map_err(|error| DeferredError::Service { op, error })
}

/// Applies one queued action against the live `backend`. `what` labels a
/// decode error from a [`DeferredAction::Popover`]/
/// [`DeferredAction::LayoutDialog`] descriptor (the binding's own name for
/// its `show_overlay` call).
pub fn apply_deferred_action(
    action: DeferredAction,
    backend: &mut dyn Backend,
    engine: &mut InteractionEngine,
    host: &mut BindingHost,
    what: &str,
) -> Result<(), DeferredError> {
    match action {
        DeferredAction::MessageDialog(opts) => {
            engine.open_message_dialog(opts, backend);
        }
        DeferredAction::ContextMenu(items, position) => {
            engine.open_context_menu(items, position, backend);
        }
        DeferredAction::Toast(toast, corner) => {
            engine.show_toast(toast, corner);
        }
        DeferredAction::Tooltip(tooltip, anchor) => {
            engine.show_tooltip(tooltip, anchor);
        }
        DeferredAction::Completions(completions, cursor) => {
            engine.show_completions(completions, cursor);
        }
        DeferredAction::Float(float, measure) => {
            engine.show_float(float, measure, backend);
        }
        DeferredAction::RichTextPopup(popup, anchor) => {
            engine.show_rich_text_popup(popup, anchor);
        }
        DeferredAction::Popover(value, measure) => {
            let decoded = {
                let theme = backend.theme();
                let styles = StyleContext::new(&host.style_sets, &theme);
                decode_popover(what, &value, styles, &host.wire_diagnostics)
            }
            .map_err(DeferredError::Decode)?;
            engine.show_popover_layout(decoded.chrome, measure, decoded.content, backend);
        }
        DeferredAction::LayoutDialog(value, size) => {
            let decoded = {
                let theme = backend.theme();
                let styles = StyleContext::new(&host.style_sets, &theme);
                decode_layout_dialog(what, &value, styles, &host.wire_diagnostics)
            }
            .map_err(DeferredError::Decode)?;
            engine.open_layout_dialog(decoded.chrome, decoded.content, size, backend);
        }
        DeferredAction::DismissOverlay(id) => match id.as_str() {
            "context_menu" => {
                engine.dismiss_context_menu(backend);
            }
            "message_dialog" => {
                engine.close_message_dialog(backend);
            }
            "tooltip" => {
                engine.hide_tooltip();
            }
            "completions" => {
                engine.dismiss_completions();
            }
            // A popover is a float this engine paints content into, so
            // either sentinel closes it.
            "float" | "popover" => {
                engine.dismiss_float(backend);
            }
            "layout_dialog" => {
                engine.close_layout_dialog(backend);
            }
            "rich_text_popup" => {
                engine.dismiss_rich_text_popup();
            }
            toast_id => {
                engine.dismiss_toast(WidgetId::new(toast_id));
            }
        },
        DeferredAction::Notification(notification) => backend_call(
            "send_notification",
            backend.services().send_notification(notification),
        )?,
        DeferredAction::OpenPath(path) => {
            backend_call("open_path", backend.services().open_path(&path))?
        }
        DeferredAction::RevealInFileManager(path) => backend_call(
            "reveal_in_file_manager",
            backend.services().reveal_in_file_manager(&path),
        )?,
        DeferredAction::MoveToTrash(path) => {
            backend_call("move_to_trash", backend.services().move_to_trash(&path))?
        }
        DeferredAction::RestoreFromTrash(path) => backend_call(
            "restore_from_trash",
            backend.services().restore_from_trash(&path),
        )?,
        DeferredAction::Title(title) => {
            backend_call("set_title", with_window(backend, |w| w.set_title(&title)))?
        }
        DeferredAction::DocumentEdited(edited) => backend_call(
            "set_document_edited",
            with_window(backend, |w| w.set_document_edited(edited)),
        )?,
        DeferredAction::RepresentedFilename(path) => backend_call(
            "set_represented_filename",
            with_window(backend, |w| w.set_represented_filename(path.as_deref())),
        )?,
        DeferredAction::TrayIcon(icon) => {
            backend_call("set_tray_icon", with_tray(backend, |t| t.set_icon(icon)))?
        }
        DeferredAction::TrayTooltip(tooltip) => backend_call(
            "set_tray_tooltip",
            with_tray(backend, |t| t.set_tooltip(&tooltip)),
        )?,
        DeferredAction::TrayMenu(items) => {
            let menu = menu_for_surface(WidgetId::new("qui-tray-menu"), items);
            backend_call("set_tray_menu", with_tray(backend, |t| t.set_menu(&menu)))?
        }
        DeferredAction::Cursor(shape) => {
            backend.set_cursor(shape);
        }
        DeferredAction::Theme(overrides) => {
            let mut value =
                serde_json::to_value(backend.theme()).expect("Theme serializes to a JSON object");
            let obj = value
                .as_object_mut()
                .expect("Theme serializes to a JSON object");
            for (key, json) in overrides {
                obj.insert(key, json);
            }
            let theme: Theme = serde_json::from_value(value)
                .expect("theme overrides are validated when they are queued");
            backend.set_theme(theme);
        }
        DeferredAction::ApplyUpdateOnQuit(handle) => service_call(
            "apply_on_quit",
            update_apply_on_quit(backend.services(), handle),
        )?,
        DeferredAction::EndPowerSaveBlock(handle) => service_call(
            "end_power_save_block",
            end_power_save_block(backend.services(), handle),
        )?,
        DeferredAction::SetLoginItem {
            enabled,
            open_hidden,
            args,
        } => service_call(
            "set_login_item",
            set_login_item(backend.services(), enabled, open_hidden, args),
        )?,
        DeferredAction::SetAsDefaultProtocolClient(scheme) => service_call(
            "set_as_default_protocol_client",
            set_as_default_protocol_client(backend.services(), &scheme),
        )?,
        DeferredAction::RemoveAsDefaultProtocolClient(scheme) => service_call(
            "remove_as_default_protocol_client",
            remove_as_default_protocol_client(backend.services(), &scheme),
        )?,
        DeferredAction::BeginDragOut { files, text } => service_call(
            "begin_drag_out",
            begin_drag_out(backend.services(), files, text),
        )?,
        DeferredAction::AddRecentDocument(path) => service_call(
            "add_recent_document",
            add_recent_document(backend.services(), &path.to_string_lossy()),
        )?,
        DeferredAction::ClearRecentDocuments => service_call(
            "clear_recent_documents",
            clear_recent_documents(backend.services()),
        )?,
        DeferredAction::SetDockMenu(items) => {
            service_call("set_dock_menu", set_dock_menu(backend.services(), items))?
        }
        DeferredAction::SetJumpList(items) => {
            service_call("set_jump_list", set_jump_list(backend.services(), items))?
        }
        DeferredAction::ActivateTab(id, index) => {
            apply_tab_activated(engine.widget_state(), id, index);
        }
        DeferredAction::DefineStyle(name, overrides) => {
            host.style_sets.define(name, *overrides);
            backend.request_frame_in(Duration::ZERO);
        }
        DeferredAction::SetTreeTableChildren(id, node_id, children) => {
            engine.set_tree_table_children(id, &node_id, children);
        }
        DeferredAction::SetTreeTableSortOverride(id, nodes) => {
            engine.set_tree_table_sort_override(id, nodes);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quadraui::tui::TuiBackend;

    fn backend() -> TuiBackend {
        TuiBackend::new()
    }

    #[test]
    fn a_failing_backend_call_reports_its_op() {
        let mut backend = backend();
        let mut engine = InteractionEngine::default();
        let mut host = BindingHost::default();
        let err = apply_deferred_action(
            DeferredAction::TrayTooltip("t".into()),
            &mut backend,
            &mut engine,
            &mut host,
            "test",
        )
        .expect_err("the TUI backend has no tray");
        assert!(matches!(
            err,
            DeferredError::Backend {
                op: "set_tray_tooltip",
                ..
            }
        ));
    }

    #[test]
    fn a_failing_service_helper_reports_its_op() {
        let mut backend = backend();
        let mut engine = InteractionEngine::default();
        let mut host = BindingHost::default();
        let err = apply_deferred_action(
            DeferredAction::ClearRecentDocuments,
            &mut backend,
            &mut engine,
            &mut host,
            "test",
        )
        .expect_err("the headless backend has no recent-documents service");
        assert!(matches!(
            err,
            DeferredError::Service {
                op: "clear_recent_documents",
                ..
            }
        ));
    }

    #[test]
    fn a_malformed_popover_is_a_decode_error() {
        let mut backend = backend();
        let mut engine = InteractionEngine::default();
        let mut host = BindingHost::default();
        let err = apply_deferred_action(
            DeferredAction::Popover(
                serde_json::json!({"float": {}}),
                FloatMeasure::new(10.0, 4.0),
            ),
            &mut backend,
            &mut engine,
            &mut host,
            "show_overlay",
        )
        .expect_err("a popover without content does not decode");
        assert!(matches!(err, DeferredError::Decode(_)));
    }

    #[test]
    fn decode_overlay_returns_each_kinds_dismiss_id() {
        let menu = serde_json::json!({"items": []});
        let (action, id) = decode_overlay("t", "context_menu", &menu, Some([1.0, 2.0, 0.0, 0.0]))
            .expect("decodes");
        assert!(matches!(action, DeferredAction::ContextMenu(_, p) if p == Point::new(1.0, 2.0)));
        assert_eq!(id, "context_menu");

        let tip = serde_json::json!({"id": "tip:1", "text": "hi"});
        let (action, id) =
            decode_overlay("t", "tooltip", &tip, Some([1.0, 2.0, 3.0, 4.0])).expect("decodes");
        assert!(
            matches!(action, DeferredAction::Tooltip(_, r) if r == Rect::new(1.0, 2.0, 3.0, 4.0))
        );
        assert_eq!(id, "tip:1");

        let popover = serde_json::json!({"float": {"id": "pop:1"}, "content": {}});
        let (_, id) =
            decode_overlay("t", "popover", &popover, Some([10.0, 4.0, 0.0, 0.0])).expect("decodes");
        assert_eq!(id, "pop:1");
    }

    #[test]
    fn decode_overlay_rejects_an_unknown_kind_and_a_missing_anchor() {
        let err = decode_overlay("show_overlay", "nope", &serde_json::json!({}), None)
            .err()
            .expect("unknown kind");
        assert!(
            err.starts_with("show_overlay: unknown overlay kind \"nope\""),
            "{err}"
        );
        let tip = serde_json::json!({"id": "tip:1", "text": "hi"});
        let err = decode_overlay("show_overlay", "tooltip", &tip, None)
            .err()
            .expect("tooltip needs an anchor");
        assert!(err.contains("needs an anchor"), "{err}");
    }

    #[test]
    fn overlay_anchor_from_value_reads_the_shape_each_kind_expects() {
        let point = serde_json::json!({"x": 3.0, "y": 4.0});
        assert_eq!(
            overlay_anchor_from_value("t", "completions", &point),
            Ok([3.0, 4.0, 0.0, 0.0])
        );
        let size = serde_json::json!({"width": 20.0, "height": 6.0});
        assert_eq!(
            overlay_anchor_from_value("t", "float", &size),
            Ok([20.0, 6.0, 0.0, 0.0])
        );
        assert!(overlay_anchor_from_value("t", "float", &point).is_err());
    }

    #[test]
    fn a_defined_style_is_registered_on_the_host() {
        let mut backend = backend();
        let mut engine = InteractionEngine::default();
        let mut host = BindingHost::default();
        apply_deferred_action(
            DeferredAction::DefineStyle("accent".into(), Box::default()),
            &mut backend,
            &mut engine,
            &mut host,
            "test",
        )
        .expect("defining a style cannot fail");
        assert!(host
            .style_sets
            .get("accent", &quadraui::Theme::default())
            .is_some());
    }
}
