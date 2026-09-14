//! `cargo run --example gtk_platform_services --features gtk`
//!
//! Manual smoke test for issue #955's GTK `send_notification`: before
//! this issue `GtkPlatformServices::send_notification` was `fn
//! send_notification(&self, _n: Notification) {}` — an empty no-op, the
//! Linux desktop backend had no notifications at all. This fires a real
//! `gio::Notification` (through the window's `gtk4::Application`) with a
//! tag and one action button, and reports
//! `UiEvent::NotificationActivated` to stderr when the user clicks the
//! notification body or its action button — GTK is the only backend
//! that delivers that event today (see that variant's own doc for why
//! macOS/Win-GUI don't yet).
//!
//! Also covers issue #956's four `shell.*`-style methods
//! (`reveal_in_file_manager`, `open_path`, `move_to_trash`, `beep`) —
//! each has a real, visible side effect (a file manager window opens; the
//! default app for a file launches; a temp file disappears from disk; a
//! sound plays), which is exactly why these are manual controls here
//! rather than automated `#[cfg(test)]` assertions in `gtk::services`
//! itself (see that module's own tests for the pieces that *are* safe to
//! assert on headlessly).
//!
//! Draws nothing — same minimal "services only, report to stderr"
//! pattern `examples/win_platform_services.rs` uses, since this is a
//! platform-services smoke test, not a visual primitive demo.
//!
//! Controls:
//! - `n` — fire a tagged notification with one action button ("Show")
//! - `r` — `reveal_in_file_manager` a freshly-written temp file (watch
//!   for a file-manager window to open with it selected)
//! - `o` — `open_path` that same temp file with its default handler
//! - `t` — write a *second* temp file and `move_to_trash` it — check
//!   your trash/recycle bin afterward
//! - `b` — `beep`
//! - `Esc` / `q` — quit
//!
//! Click the fired notification (its body, or the "Show" button) in
//! your desktop's notification area/shell to see the resulting
//! `NotificationActivated` event reported here.

use quadraui::{AppLogic, Backend, Key, NamedKey, Notification, Reaction, UiEvent, WidgetId};

/// Write a throwaway temp file for `r`/`o`/`t` to act on, so this demo
/// never touches anything the user actually cares about.
fn scratch_file(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "quadraui-956-gtk-platform-services-demo-{label}-{}.txt",
        std::process::id()
    ));
    let _ = std::fs::write(&path, b"quadraui#956 demo scratch file");
    path
}

struct PlatformServicesDemo;

impl AppLogic for PlatformServicesDemo {
    type AreaId = ();

    fn render(&self, _backend: &mut dyn Backend, _area: ()) {
        // Nothing to paint — see module docs above.
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::WindowClose => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape) | Key::Char('q'),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Char('n'),
                ..
            } => {
                backend.services().send_notification(
                    Notification::new("quadraui", "#955 platform services demo")
                        .with_tag("gtk-platform-services-demo")
                        .with_action(WidgetId::new("show"), "Show"),
                );
                eprintln!(
                    "notification fired — click it (or its \"Show\" button) to see \
                     NotificationActivated reported here"
                );
                Reaction::Continue
            }
            UiEvent::NotificationActivated { tag, action } => {
                eprintln!("NotificationActivated: tag={tag:?} action={action:?}");
                Reaction::Continue
            }
            UiEvent::KeyPressed {
                key: Key::Char('r'),
                ..
            } => {
                let path = scratch_file("reveal");
                match backend.services().reveal_in_file_manager(&path) {
                    Ok(()) => {
                        eprintln!("reveal_in_file_manager: opened a file manager at {path:?}")
                    }
                    Err(e) => eprintln!("reveal_in_file_manager FAILED: {e:?}"),
                }
                Reaction::Continue
            }
            UiEvent::KeyPressed {
                key: Key::Char('o'),
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
                key: Key::Char('t'),
                ..
            } => {
                let path = scratch_file("trash");
                match backend.services().move_to_trash(&path) {
                    Ok(()) => eprintln!("move_to_trash: moved {path:?} to the trash"),
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
            _ => Reaction::Continue,
        }
    }
}

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(PlatformServicesDemo)
}
