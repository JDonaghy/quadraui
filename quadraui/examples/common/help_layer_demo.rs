//! Backend-agnostic `ShellApp` for the context-sensitive help layer demo
//! (#431 — [`tui_help_layer`] / [`gtk_help_layer`]).
//!
//! Two panels ("Explorer" and "Source Control"), each registered with its
//! own [`quadraui::HelpRegistry`] entry — different notes, different
//! actions. Demonstrates the full #431 acceptance shape:
//!
//! - `?` opens a cheatsheet overlay ([`quadraui::HelpOverlayController`])
//!   showing the **active panel's** registered notes + actions — switch
//!   panels, reopen `?`, and the content changes. That's the
//!   "context-sensitive" part.
//! - `p` opens a command palette ([`quadraui::DualModePaletteController`])
//!   populated from the active panel's registered actions via
//!   [`quadraui::help_actions_to_palette_items`]; typing filters by label
//!   **and** description via [`quadraui::filter_help_actions`].
//!
//! Controls:
//! - `?`                 toggle the help cheatsheet for the active panel
//! - `p`                 open the command palette (Esc to cancel, Enter to run)
//! - `Tab`                focus the activity bar (j/k/Enter/Esc to navigate panels)
//! - `q` / `Esc`          quit (when no overlay is open)

use quadraui::compose::app_shell::{AppShellEvent, AppShellLayout, PanelDefinition};
use quadraui::{
    filter_help_actions, help_actions_to_palette_items, Backend, Color, DualModePaletteController,
    DualModePaletteEvent, HelpAction, HelpNote, HelpOverlayController, HelpOverlayEvent,
    HelpRegistry, Key, NamedKey, Reaction, Rect, ShellApp, ShellConfig, ShellContext, StatusBar,
    StatusBarSegment, UiEvent, ViewHelp, WidgetId,
};

const EXPLORER_PANEL: &str = "panel:explorer";
const GIT_PANEL: &str = "panel:git";

pub struct HelpLayerDemo {
    registry: HelpRegistry,
    help_overlay: HelpOverlayController,
    palette: Option<DualModePaletteController>,
    active_panel: WidgetId,
    last_message: String,
}

impl HelpLayerDemo {
    pub fn new() -> Self {
        let mut registry = HelpRegistry::new();
        registry.register(
            EXPLORER_PANEL,
            ViewHelp::new("Explorer")
                .with_notes(vec![
                    HelpNote::new("●", "File has unsaved changes"),
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

        Self {
            registry,
            help_overlay: HelpOverlayController::new(),
            palette: None,
            active_panel: WidgetId::new(EXPLORER_PANEL),
            last_message: "?=help  p=commands  Tab=focus bar  q=quit".into(),
        }
    }

    pub fn config() -> ShellConfig {
        ShellConfig::new(
            "Help Layer Demo",
            vec![
                PanelDefinition {
                    id: WidgetId::new(EXPLORER_PANEL),
                    icon: "E".into(),
                    tooltip: "Explorer".into(),
                    title: "EXPLORER".into(),
                },
                PanelDefinition {
                    id: WidgetId::new(GIT_PANEL),
                    icon: "G".into(),
                    tooltip: "Source Control".into(),
                    title: "SOURCE CONTROL".into(),
                },
            ],
        )
    }

    fn active_view_help(&self) -> Option<&ViewHelp> {
        self.registry.get(self.active_panel.as_str())
    }

    /// Rebuild the palette's item list from the active panel's actions,
    /// filtered by `query` — mirrors the recompute-on-query pattern in
    /// `examples/common/palette_dual_mode_app.rs` so a selected index maps
    /// back to the same `HelpAction`.
    fn filtered_actions(&self, query: &str) -> Vec<&HelpAction> {
        filter_help_actions(self.registry.actions_for(self.active_panel.as_str()), query)
    }

    fn open_palette(&mut self) {
        let items = help_actions_to_palette_items(self.filtered_actions(""));
        self.palette = Some(DualModePaletteController::new("Commands", None, items));
        self.last_message =
            "Command palette open — type to search, Enter to run, Esc to cancel".into();
    }
}

impl Default for HelpLayerDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellApp for HelpLayerDemo {
    fn render_content(&self, backend: &mut dyn Backend, layout: &AppShellLayout) {
        let lh = backend.line_height();

        let hint = StatusBar {
            id: WidgetId::new("main-label"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_message),
                fg: Color::rgb(200, 200, 200),
                bg: Color::rgb(30, 30, 30),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.active_panel.as_str()),
                fg: Color::rgb(140, 200, 255),
                bg: Color::rgb(20, 40, 70),
                bold: true,
                action_id: None,
            }],
        };
        let rect = Rect::new(
            layout.main_content_bounds.x,
            layout.main_content_bounds.y,
            layout.main_content_bounds.width,
            lh,
        );
        backend.draw_status_bar(rect, &hint, None, None);

        if let Some(palette) = &self.palette {
            let popup = popup_rect(layout.window_bounds, backend);
            palette.render(popup, backend);
        } else if let Some(help) = self.active_view_help() {
            self.help_overlay
                .render(layout.window_bounds, backend, help);
        }
    }

    fn handle(
        &mut self,
        event: UiEvent,
        backend: &mut dyn Backend,
        ctx: &ShellContext,
    ) -> Reaction {
        // Palette open → it owns every key until it closes. `.take()` so
        // `self.filtered_actions()` below can borrow `self` immutably
        // without conflicting with an outstanding `&mut self.palette`.
        if let Some(mut palette) = self.palette.take() {
            let popup = popup_rect(ctx.window_bounds(), backend);
            let lh = backend.line_height();
            let visible_rows = if lh > 0.0 {
                ((popup.height / lh) as usize).saturating_sub(quadraui::PALETTE_CHROME_ROWS)
            } else {
                10
            };
            let reaction = match palette.handle(&event, visible_rows) {
                DualModePaletteEvent::ItemConfirmed { idx } => {
                    let query = palette.query().to_string();
                    let matched = self.filtered_actions(&query);
                    if let Some(action) = matched.get(idx) {
                        self.last_message = format!("Ran: {}", action.label);
                    }
                    Reaction::Redraw
                }
                DualModePaletteEvent::QueryChanged { value } => {
                    let matched = self.filtered_actions(&value);
                    palette.set_items(help_actions_to_palette_items(matched));
                    self.palette = Some(palette);
                    return Reaction::Redraw;
                }
                DualModePaletteEvent::Cancelled => {
                    self.last_message = "Command palette closed".into();
                    Reaction::Redraw
                }
                DualModePaletteEvent::TextConfirmed { .. }
                | DualModePaletteEvent::ModeToggled { .. }
                | DualModePaletteEvent::Consumed => {
                    self.palette = Some(palette);
                    return Reaction::Redraw;
                }
                DualModePaletteEvent::Ignored => {
                    self.palette = Some(palette);
                    return Reaction::Continue;
                }
            };
            // Reached only for ItemConfirmed / Cancelled — palette closes.
            return reaction;
        }

        // Help overlay: `?` opens/closes when relevant, swallows keys
        // while open, `Ignored` when closed and the key isn't `?`.
        match self.help_overlay.handle(&event) {
            HelpOverlayEvent::Opened => {
                self.last_message = format!("Help — {}", self.active_panel.as_str());
                return Reaction::Redraw;
            }
            HelpOverlayEvent::Closed => {
                self.last_message = "Help closed".into();
                return Reaction::Redraw;
            }
            HelpOverlayEvent::Consumed => return Reaction::Redraw,
            HelpOverlayEvent::Ignored => {}
        }

        match event {
            UiEvent::KeyPressed {
                key: Key::Char('p'),
                ..
            } => {
                self.open_palette();
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                ..
            } => {
                ctx.request_activity_keyboard_focus();
                self.last_message = "Activity bar focused (j/k, Enter, Esc)".into();
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('q') | Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            _ => Reaction::Continue,
        }
    }

    fn on_shell_event(&mut self, event: &AppShellEvent) {
        if let AppShellEvent::PanelChanged { panel_id } = event {
            self.active_panel = panel_id.clone();
            self.last_message = format!("Panel: {}", panel_id.as_str());
        }
    }
}

/// Centered popup geometry — 70% of `container`, floored to a readable
/// minimum via `backend.line_height()` / `char_width()` (never a
/// hardcoded cell/pixel constant — see `docs/LESSONS.md`).
fn popup_rect(container: Rect, backend: &dyn Backend) -> Rect {
    let lh = backend.line_height();
    let cw = backend.char_width();
    let w = (container.width * 0.6).max(cw * 40.0).min(container.width);
    let h = (container.height * 0.6)
        .max(lh * 10.0)
        .min(container.height);
    let x = container.x + (container.width - w) * 0.5;
    let y = container.y + (container.height - h) * 0.5;
    Rect::new(x, y, w, h)
}
