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
//! Draws nothing — same minimal "services only, report to stderr"
//! pattern `examples/win_platform_services.rs` uses, since this is a
//! platform-services smoke test, not a visual primitive demo.
//!
//! Controls:
//! - `n` — fire a tagged notification with one action button ("Show")
//! - `Esc` / `q` — quit
//!
//! Click the fired notification (its body, or the "Show" button) in
//! your desktop's notification area/shell to see the resulting
//! `NotificationActivated` event reported here.

use quadraui::{AppLogic, Backend, Key, NamedKey, Notification, Reaction, UiEvent, WidgetId};

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
            _ => Reaction::Continue,
        }
    }
}

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(PlatformServicesDemo)
}
