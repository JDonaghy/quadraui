//! `cargo run --example win_platform_services --features win` (Windows only)
//!
//! Manual smoke test for issue #23's `WinPlatformServices`: clipboard
//! round-trip, native file open/save dialogs, a `Shell_NotifyIconW`
//! balloon notification, and `open_url`.
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
//! - `u` — `open_url` a fixed address (the default browser should launch)
//! - `Esc` / `q` — quit
//!
//! `quadraui::win::run` only exists when compiled for `target_os =
//! "windows"` (see `src/win/mod.rs`/`Cargo.toml`'s `win` feature
//! comment) — this example is Windows-only, same posture as `win_demo`
//! (see that example's module docs).

#[cfg(target_os = "windows")]
use quadraui::{
    AppLogic, Backend, FileDialogOptions, Key, NamedKey, Notification, Reaction, UiEvent,
};

#[cfg(target_os = "windows")]
struct PlatformServicesDemo;

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
                backend.services().send_notification(Notification {
                    title: "quadraui".to_string(),
                    body: "#23 platform services demo".to_string(),
                    urgent: false,
                });
                eprintln!("notification fired — check the notification area");
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
