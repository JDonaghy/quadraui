//! Platform-services seam demo — exercises `Backend::services()` /
//! `PlatformServices` from app code and shows the `ServiceResult` each call
//! returns as on-screen status text.
//!
//! Every key below reaches a `PlatformServices` method directly (unlike
//! `WindowControlDemo`'s `window()`, which returns
//! `Option<&mut dyn WindowControl>` and so has a "no window yet" case to
//! fold in, `services()` returns `&dyn PlatformServices` directly with no
//! such case). TUI gives three of these real, honest answers (`app_path`,
//! the power-save-block round trip, `login_item_enabled`) and reports
//! `Unsupported` for the rest — there is no OS global-shortcut registry,
//! drag source, login-item registry, protocol-handler registry,
//! recent-documents list, dock menu, jump list, or updater a terminal
//! controls. `system_theme` is a fourth, host-dependent case: it reports
//! the real `SystemTheme` (including `reduce_motion`) when the terminal's
//! environment exposes one, and `Unsupported` otherwise — both outcomes
//! are deterministic and driver-testable with no live desktop required.
//!
//! Deliberately out of scope: `install_crash_handler` (a process-global
//! `std::panic::set_hook` with no backend-specific honesty story — wiring
//! it here would install a panic hook in the test binary itself) and
//! `set_document_edited`/`set_represented_filename` (methods on
//! `WindowControl`, reached via `backend.window()`, not `PlatformServices`
//! — already covered by `WindowControlDemo`).
//!
//! Keys: `p` app_path, `t` system_theme, `w` power-save-block round trip,
//! `g` register_global_shortcut, `d` begin_drag_out, `l` set_login_item,
//! `e` login_item_enabled, `o` set_as_default_protocol_client, `r`
//! add_recent_document, `c` clear_recent_documents, `k` set_dock_menu, `j`
//! set_jump_list, `u` update_service().download_and_stage. Esc quits.

use std::fmt::Debug;
use std::path::PathBuf;

use quadraui::backend::JumpListItem;
use quadraui::{
    Accelerator, AcceleratorId, AcceleratorScope, AppLogic, AppPathKind, Backend, Color,
    ContextMenu, ContextMenuPlacement, DragOutPayload, InteractionState, Key, KeyBinding,
    LoginItemOptions, NamedKey, PowerSaveKind, Reaction, Rect, ServiceResult, StatusBar,
    StatusBarSegment, UiEvent, UpdateInfo, WidgetId,
};

pub struct PlatformSeamsDemo {
    /// One hint per key binding, lowest priority first. `StatusBar`'s
    /// priority-drop layout always keeps the last entry visible even when it
    /// alone overflows the available width, so `"p=path"` sits last to stay
    /// the one guaranteed-visible hint at any terminal width.
    status: Vec<String>,
}

impl PlatformSeamsDemo {
    pub fn new() -> Self {
        Self {
            status: vec![
                "t=theme".to_string(),
                "w=power-block".to_string(),
                "g=global-shortcut".to_string(),
                "d=drag-out".to_string(),
                "l=login-item".to_string(),
                "e=login-item-enabled".to_string(),
                "o=protocol".to_string(),
                "r=add-recent".to_string(),
                "c=clear-recent".to_string(),
                "k=dock-menu".to_string(),
                "j=jump-list".to_string(),
                "u=update".to_string(),
                "p=path".to_string(),
            ],
        }
    }

    /// Renders any `ServiceResult<T>` as status text, for whichever `Ok`
    /// type the call in question returns.
    fn describe<T: Debug>(action: &str, result: ServiceResult<T>) -> String {
        match result {
            Ok(v) => format!("{action}: {v:?}"),
            Err(e) => format!("{action}: {e:?}"),
        }
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("platform-seams-demo:status"),
            left_segments: vec![StatusBarSegment {
                text: " Platform services demo ".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: self
                .status
                .iter()
                .map(|s| StatusBarSegment {
                    text: format!(" {} ", s),
                    fg: Color::rgb(220, 220, 220),
                    bg: Color::rgb(40, 80, 120),
                    bold: false,
                    action_id: None,
                })
                .collect(),
        }
    }
}

impl Default for PlatformSeamsDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for PlatformSeamsDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Char('p'),
                ..
            } => {
                let result = backend.services().app_path(AppPathKind::UserData);
                self.status = vec![Self::describe("app_path", result)];
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('t'),
                ..
            } => {
                let result = backend.services().system_theme();
                self.status = vec![Self::describe("system_theme", result)];
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('w'),
                ..
            } => {
                let result = backend
                    .services()
                    .begin_power_save_block(PowerSaveKind::PreventDisplaySleep)
                    .and_then(|id| backend.services().end_power_save_block(id));
                self.status = vec![Self::describe("power_save_block", result)];
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('g'),
                ..
            } => {
                let accel = Accelerator {
                    id: AcceleratorId::new("platform-seams-demo.summon"),
                    binding: KeyBinding::Literal("<C-S-space>".to_string()),
                    scope: AcceleratorScope::Global,
                    label: None,
                };
                let result = backend.services().register_global_shortcut(accel);
                self.status = vec![Self::describe("register_global_shortcut", result)];
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('d'),
                ..
            } => {
                let payload = DragOutPayload::new().with_text("platform-seams-demo");
                let result = backend.services().begin_drag_out(payload);
                self.status = vec![Self::describe("begin_drag_out", result)];
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('l'),
                ..
            } => {
                let result = backend
                    .services()
                    .set_login_item(true, LoginItemOptions::new());
                self.status = vec![Self::describe("set_login_item", result)];
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('e'),
                ..
            } => {
                let result = backend.services().login_item_enabled();
                self.status = vec![Self::describe("login_item_enabled", result)];
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('o'),
                ..
            } => {
                let result = backend
                    .services()
                    .set_as_default_protocol_client("platform-seams-demo");
                self.status = vec![Self::describe("set_as_default_protocol_client", result)];
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('r'),
                ..
            } => {
                let path = PathBuf::from("/tmp/platform-seams-demo.txt");
                let result = backend.services().add_recent_document(&path);
                self.status = vec![Self::describe("add_recent_document", result)];
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('c'),
                ..
            } => {
                let result = backend.services().clear_recent_documents();
                self.status = vec![Self::describe("clear_recent_documents", result)];
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('k'),
                ..
            } => {
                let menu = ContextMenu {
                    id: WidgetId::new("platform-seams-demo:dock-menu"),
                    items: Vec::new(),
                    selected_idx: 0,
                    bg: None,
                    placement: ContextMenuPlacement::AnchorPoint,
                };
                let result = backend.services().set_dock_menu(&menu);
                self.status = vec![Self::describe("set_dock_menu", result)];
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('j'),
                ..
            } => {
                let items = [JumpListItem::new().with_title("Recent project")];
                let result = backend.services().set_jump_list(&items);
                self.status = vec![Self::describe("set_jump_list", result)];
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('u'),
                ..
            } => {
                let info = UpdateInfo::new(
                    "1.2.3",
                    "bug fixes",
                    "https://example.invalid/update.bin",
                    "sig",
                );
                let result = backend
                    .services()
                    .update_service()
                    .download_and_stage(&info);
                self.status = vec![Self::describe("download_and_stage", result)];
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
