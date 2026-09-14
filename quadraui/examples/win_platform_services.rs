//! `cargo run --example win_platform_services --features win` (Windows only)
//!
//! Manual smoke test for issue #23's `WinPlatformServices`: clipboard
//! round-trip, native file open/save dialogs, a `Shell_NotifyIconW`
//! balloon notification, and `open_url` — plus issue #744's
//! `TaskDialogIndirect`-backed message dialog.
//!
//! Draws nothing — like `win_demo`, every `WinBackend::draw_*` rasteriser
//! is still a `todo!()` stub (no `draw_status_bar` to report results
//! in-window with), so this exercises `backend.services()` directly from
//! `AppLogic::handle` and reports results to stderr — the console this
//! example is launched from — instead of an in-window status bar. Once a
//! later issue lands the Direct2D status-bar rasteriser, this can be
//! rewritten to share `examples/common/file_dialog_demo.rs`'s in-window
//! status the way `gtk_file_dialog`/`tui_file_dialog` already do.
//!
//! Controls:
//! - `c` — write a fixed string to the clipboard, then read it back and
//!   report whether it round-tripped
//! - `o` — native file-open dialog (filtered to `*.rs`)
//! - `s` — native file-save dialog (initial name `untitled.txt`)
//! - `n` — fire a balloon notification (watch the notification area)
//! - `m` — native message dialog (`TaskDialogIndirect`, #744) with
//!   Save/Don't Save/Cancel buttons; reports which one was chosen
//! - `u` — `open_url` a fixed address (the default browser should launch)
//! - `t` — query the OS light/dark/accent/high-contrast preference
//!   (`system_theme`, #952) and report it; flip Windows' Settings ->
//!   Personalization -> Colors "Choose your mode" between runs to see the
//!   answer change
//! - `f` — `reveal_in_file_manager` (#956) a freshly-written temp file —
//!   an Explorer window should open with it selected
//! - `p` — `open_path` (#956) that same temp file with its default
//!   handler
//! - `x` — write a *second* temp file and `move_to_trash` (#956) it —
//!   check the Recycle Bin afterward
//! - `b` — `beep` (#956)
//! - `d` — issue #959's `displays`/`cursor_screen_point`: prints every
//!   connected monitor (bounds, work area, scale, primary) and the
//!   current cursor position to stderr — move the window to a second
//!   monitor and press `d` again to see the list/cursor position change
//! - `Esc` / `q` — quit
//!
//! `quadraui::win::run` only exists when compiled for `target_os =
//! "windows"` (see `src/win/mod.rs`/`Cargo.toml`'s `win` feature
//! comment) — this example is Windows-only, same posture as `win_demo`
//! (see that example's module docs).

#[cfg(target_os = "windows")]
use quadraui::{
    AppLogic, Backend, DialogSeverity, FileDialogOptions, Key, MessageDialogButton,
    MessageDialogOptions, NamedKey, Notification, Reaction, UiEvent, WidgetId,
};

#[cfg(target_os = "windows")]
struct PlatformServicesDemo;

/// Write a throwaway temp file for `f`/`p`/`x` to act on, so this demo
/// never touches anything the user actually cares about (issue #956).
#[cfg(target_os = "windows")]
fn scratch_file(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "quadraui-956-win-platform-services-demo-{label}-{}.txt",
        std::process::id()
    ));
    let _ = std::fs::write(&path, b"quadraui#956 demo scratch file");
    path
}

#[cfg(target_os = "windows")]
impl AppLogic for PlatformServicesDemo {
    type AreaId = ();

    fn render(&self, _backend: &mut dyn Backend, _area: ()) {
        // Nothing to paint yet (see module docs above) — `begin_frame`'s
        // `Clear` is the entire visible content of this demo, same as
        // `win_demo`.
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::WindowClose => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape) | Key::Char('q'),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Char('c'),
                ..
            } => {
                const PAYLOAD: &str = "quadraui #23 clipboard round-trip";
                let clipboard = backend.services().clipboard();
                clipboard.write_text(PAYLOAD);
                match clipboard.read_text() {
                    Some(text) if text == PAYLOAD => {
                        eprintln!("clipboard round-trip OK: {text:?}");
                    }
                    Some(text) => eprintln!("clipboard round-trip MISMATCH: got {text:?}"),
                    None => eprintln!("clipboard round-trip FAILED: nothing read back"),
                }
                Reaction::Continue
            }
            UiEvent::KeyPressed {
                key: Key::Char('o'),
                ..
            } => {
                let opts = FileDialogOptions {
                    title: Some("Open File".to_string()),
                    filters: vec![("Rust files".to_string(), vec!["rs".to_string()])],
                    ..Default::default()
                };
                match backend.services().show_file_open_dialog(opts) {
                    Some(path) => eprintln!("opened: {}", path.display()),
                    None => eprintln!("open cancelled"),
                }
                Reaction::Continue
            }
            UiEvent::KeyPressed {
                key: Key::Char('s'),
                ..
            } => {
                let opts = FileDialogOptions {
                    title: Some("Save As".to_string()),
                    initial_filename: Some("untitled.txt".to_string()),
                    ..Default::default()
                };
                match backend.services().show_file_save_dialog(opts) {
                    Some(path) => eprintln!("save as: {}", path.display()),
                    None => eprintln!("save cancelled"),
                }
                Reaction::Continue
            }
            UiEvent::KeyPressed {
                key: Key::Char('n'),
                ..
            } => {
                backend
                    .services()
                    .send_notification(Notification::new("quadraui", "#23 platform services demo"));
                eprintln!("notification fired — check the notification area");
                Reaction::Continue
            }
            UiEvent::KeyPressed {
                key: Key::Char('m'),
                ..
            } => {
                let opts = MessageDialogOptions {
                    title: "Save changes?".to_string(),
                    body: "quadraui #744 message dialog demo — pick a button.".to_string(),
                    buttons: vec![
                        MessageDialogButton {
                            id: WidgetId::new("save"),
                            label: "Save".to_string(),
                            is_default: true,
                            is_cancel: false,
                        },
                        MessageDialogButton {
                            id: WidgetId::new("dont_save"),
                            label: "Don't Save".to_string(),
                            is_default: false,
                            is_cancel: false,
                        },
                        MessageDialogButton {
                            id: WidgetId::new("cancel"),
                            label: "Cancel".to_string(),
                            is_default: false,
                            is_cancel: true,
                        },
                    ],
                    severity: Some(DialogSeverity::Warning),
                };
                match backend.services().show_message_dialog(opts) {
                    Some(id) => eprintln!("message dialog: chose {id:?}"),
                    None => eprintln!("message dialog: dismissed with no choice"),
                }
                Reaction::Continue
            }
            UiEvent::KeyPressed {
                key: Key::Char('u'),
                ..
            } => {
                backend.services().open_url("https://example.com");
                eprintln!("open_url called — the default browser should launch");
                Reaction::Continue
            }
            UiEvent::KeyPressed {
                key: Key::Char('t'),
                ..
            } => {
                match backend.services().system_theme() {
                    Ok(theme) => eprintln!("system_theme: {theme:?}"),
                    Err(e) => eprintln!("system_theme: unavailable ({e:?})"),
                }
                Reaction::Continue
            }
            UiEvent::KeyPressed {
                key: Key::Char('f'),
                ..
            } => {
                let path = scratch_file("reveal");
                match backend.services().reveal_in_file_manager(&path) {
                    Ok(()) => eprintln!("reveal_in_file_manager: opened Explorer at {path:?}"),
                    Err(e) => eprintln!("reveal_in_file_manager FAILED: {e:?}"),
                }
                Reaction::Continue
            }
            UiEvent::KeyPressed {
                key: Key::Char('p'),
                ..
            } => {
                let path = scratch_file("open");
                match backend.services().open_path(&path) {
                    Ok(()) => eprintln!("open_path: launched the default handler for {path:?}"),
                    Err(e) => eprintln!("open_path FAILED: {e:?}"),
                }
                Reaction::Continue
            }
            UiEvent::KeyPressed {
                key: Key::Char('x'),
                ..
            } => {
                let path = scratch_file("trash");
                match backend.services().move_to_trash(&path) {
                    Ok(()) => eprintln!("move_to_trash: moved {path:?} to the Recycle Bin"),
                    Err(e) => eprintln!("move_to_trash FAILED: {e:?}"),
                }
                Reaction::Continue
            }
            UiEvent::KeyPressed {
                key: Key::Char('b'),
                ..
            } => {
                match backend.services().beep() {
                    Ok(()) => eprintln!("beep: ok"),
                    Err(e) => eprintln!("beep FAILED: {e:?}"),
                }
                Reaction::Continue
            }
            UiEvent::KeyPressed {
                key: Key::Char('d'),
                ..
            } => {
                match backend.services().displays() {
                    Ok(displays) => {
                        for (i, d) in displays.iter().enumerate() {
                            eprintln!("display[{i}]: {d:?}");
                        }
                    }
                    Err(e) => eprintln!("displays FAILED: {e:?}"),
                }
                match backend.services().cursor_screen_point() {
                    Ok(p) => eprintln!("cursor_screen_point: {p:?}"),
                    Err(e) => eprintln!("cursor_screen_point FAILED: {e:?}"),
                }
                Reaction::Continue
            }
            _ => Reaction::Continue,
        }
    }
}

#[cfg(target_os = "windows")]
fn main() -> std::process::ExitCode {
    quadraui::win::run(PlatformServicesDemo)
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("win_platform_services only runs on Windows — see this file's module docs.");
}
