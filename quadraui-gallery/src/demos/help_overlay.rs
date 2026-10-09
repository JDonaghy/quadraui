//! `Help Overlay` demo — adapted from
//! `quadraui/examples/common/help_layer_demo.rs`.
//!
//! Three variants stand in for that example's three panels (Explorer,
//! Source Control, Settings), each with its own
//! [`quadraui::HelpRegistry`] entry — `?` opens a cheatsheet overlay
//! ([`HelpOverlayController`]) showing the **active variant's**
//! registered notes, demonstrating the "context-sensitive" contract:
//! switch variants, reopen `?`, and the content changes. The Settings
//! variant intentionally has **no** registered help, exercising
//! [`HelpOverlayController::render`]'s fallback — `?` still opens a
//! visible "no help available" cheatsheet rather than silently doing
//! nothing while still swallowing the key.
//!
//! The source example also drives a command palette from the same
//! registry's actions via `p` — that's the exact behaviour the
//! gallery's own "Command Palette" demo (`DualModePaletteController`)
//! already covers, so it isn't duplicated here; this demo is scoped to
//! [`HelpOverlayController`] itself.

use quadraui::{
    Backend, BackendCaps, Color, HelpAction, HelpNote, HelpOverlayController, HelpOverlayEvent,
    HelpRegistry, InteractionState, Reaction, Rect, StatusBar, StatusBarSegment, UiEvent, ViewHelp,
    WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("help_overlay.rs");

// gallery:begin
const EXPLORER_PANEL: &str = "gallery:help:explorer";
const GIT_PANEL: &str = "gallery:help:git";
const SETTINGS_PANEL: &str = "gallery:help:settings";

pub struct HelpOverlayDemo {
    registry: HelpRegistry,
    overlay: HelpOverlayController,
}

impl HelpOverlayDemo {
    pub fn new() -> Self {
        let mut registry = HelpRegistry::new();
        registry.register(
            EXPLORER_PANEL,
            ViewHelp::new("Explorer")
                .with_notes(vec![
                    HelpNote::new("\u{25cf}", "File has unsaved changes"),
                    HelpNote::new("M", "File modified since last commit"),
                ])
                .with_actions(vec![
                    HelpAction::new("explorer.new_file", "New File", "Create a new file")
                        .with_accelerator("Ctrl+N"),
                    HelpAction::new(
                        "explorer.reveal",
                        "Reveal in Finder",
                        "Show the selected file on disk",
                    )
                    .with_accelerator("Ctrl+Shift+R"),
                ]),
        );
        registry.register(
            GIT_PANEL,
            ViewHelp::new("Source Control")
                .with_notes(vec![
                    HelpNote::new("M", "Modified"),
                    HelpNote::new("A", "Added"),
                    HelpNote::new("U", "Untracked"),
                ])
                .with_actions(vec![
                    HelpAction::new("git.commit", "Commit", "Commit staged changes")
                        .with_accelerator("Ctrl+Enter"),
                    HelpAction::new(
                        "git.discard",
                        "Discard Changes",
                        "Revert the selected file to HEAD",
                    ),
                ]),
        );
        // `SETTINGS_PANEL` is deliberately never registered — the
        // "no help available" fallback this demo exists to show.

        Self {
            registry,
            overlay: HelpOverlayController::new().with_id("gallery:help:cheatsheet"),
        }
    }

    fn panel_id(variant: usize) -> &'static str {
        match variant {
            0 => EXPLORER_PANEL,
            1 => GIT_PANEL,
            _ => SETTINGS_PANEL,
        }
    }

    fn panel_label(variant: usize) -> &'static str {
        match variant {
            0 => "Explorer",
            1 => "Source Control",
            _ => "Settings (no help registered)",
        }
    }

    fn active_view_help(&self, variant: usize) -> Option<&ViewHelp> {
        self.registry.get(Self::panel_id(variant))
    }

    fn status_bar(variant: usize, open: bool) -> StatusBar {
        let hint = if open {
            " Esc or ? to close "
        } else {
            " ? = open help for this panel "
        };
        StatusBar {
            id: WidgetId::new("gallery:help:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" Panel: {} ", Self::panel_label(variant)),
                fg: Color::rgb(200, 200, 200),
                bg: Color::rgb(30, 30, 30),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: hint.into(),
                fg: Color::rgb(140, 200, 255),
                bg: Color::rgb(20, 40, 70),
                bold: true,
                action_id: None,
            }],
        }
    }
}

impl Default for HelpOverlayDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for HelpOverlayDemo {
    fn name(&self) -> &'static str {
        "Help Overlay"
    }

    fn group(&self) -> &'static str {
        "Overlays"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Explorer", "Source Control", "Settings (no help)"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();
        let bar_rect = Rect::new(area.x, area.y, area.width, lh);
        backend.draw_status_bar_interactive(
            bar_rect,
            &Self::status_bar(variant, self.overlay.is_open()),
            &InteractionState::new(),
        );

        self.overlay
            .render(area, backend, self.active_view_help(variant));
    }

    fn handle(
        &mut self,
        _variant: usize,
        event: &UiEvent,
        _backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        match self.overlay.handle(event) {
            HelpOverlayEvent::Opened | HelpOverlayEvent::Closed | HelpOverlayEvent::Consumed => {
                Reaction::Redraw
            }
            HelpOverlayEvent::Ignored => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        serde_json::json!({
            "panel": Self::panel_label(variant),
            "has_help": self.active_view_help(variant).is_some(),
            "overlay_open": self.overlay.is_open(),
        })
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        // `HelpOverlayController` is an in-canvas compose controller with
        // no `Backend` capability flag — it works identically on every
        // backend, including the Settings variant's no-help fallback.
        None
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_variant_has_no_registered_help() {
        let demo = HelpOverlayDemo::new();
        assert!(demo.active_view_help(2).is_none());
        assert!(demo.active_view_help(0).is_some());
        assert!(demo.active_view_help(1).is_some());
    }

    #[test]
    fn panel_label_matches_each_variant() {
        assert_eq!(HelpOverlayDemo::panel_label(0), "Explorer");
        assert_eq!(HelpOverlayDemo::panel_label(1), "Source Control");
        assert_eq!(
            HelpOverlayDemo::panel_label(2),
            "Settings (no help registered)"
        );
    }
}
