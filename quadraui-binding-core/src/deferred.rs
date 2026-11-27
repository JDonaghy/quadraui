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
