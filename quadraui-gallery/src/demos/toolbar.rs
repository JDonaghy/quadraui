//! `Toolbar` demo — adapted from `quadraui/examples/common/toolbar_app.rs`.
//!
//! Exercises action buttons, a toggle action, a permanently-disabled
//! action, a separator, a non-clickable label, hover/press state and
//! keyboard focus (Tab / Shift-Tab / Enter) — the `Toolbar` primitive's
//! full surface.

use quadraui::{
    Backend, BackendCaps, Color, Icon, InteractionState, Key, NamedKey, Reaction, Rect, StatusBar,
    StatusBarSegment, Style, Toolbar, ToolbarButton, ToolbarHit, ToolbarIcons, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("toolbar.rs");

/// `Style`'s chrome-metric `token` (`tab_height`, `status_bar_height`,
/// `toolbar_height`, …), floored at the chrome line height on a pixel
/// backend — never shrunk below it, so a chrome font taller than the
/// design token isn't clipped — or the plain chrome line height on TUI's
/// fixed cell grid, where every `Style` chrome metric is a documented
/// no-op. Mirrors `AppShell`'s own token-vs-line-height carve
/// (`compose::shell_layout::compute_layout`'s `header_h`/`status_h`).
fn chrome_strip_height(backend: &dyn Backend, token: impl Fn(&Style) -> f32) -> f32 {
    let lh = backend.chrome_line_height();
    if lh <= 1.0 {
        backend.snap_height(lh)
    } else {
        backend.snap_height(lh.max(token(&backend.style())))
    }
}

// gallery:begin
pub struct ToolbarDemo {
    filter_active: bool,
    running: bool,
    last_message: String,
    interaction: InteractionState,
    focused_index: Option<usize>,
}

impl ToolbarDemo {
    pub fn new() -> Self {
        Self {
            filter_active: false,
            running: true,
            last_message: "Click, Tab to focus, Enter to activate".into(),
            interaction: InteractionState::new(),
            focused_index: None,
        }
    }

    /// `variant == 1` drops button labels and keeps only icon + tooltip —
    /// the compact style a space-constrained host toolbar would use.
    fn toolbar(&self, variant: usize) -> Toolbar {
        let labeled = variant == 0;
        let label = |text: &str| if labeled { text.into() } else { String::new() };
        Toolbar::new(WidgetId::new("gallery:toolbar"))
            .with_buttons(vec![
                ToolbarButton::Action {
                    spec: None,
                    id: WidgetId::new("gallery:toolbar:continue"),
                    label: label("Continue"),
                    icon: Some("▶".into()),
                    key_hint: Some("1".into()),
                    enabled: !self.running,
                    is_active: false,
                    tooltip: "Resume execution".into(),
                },
                ToolbarButton::Action {
                    spec: None,
                    id: WidgetId::new("gallery:toolbar:pause"),
                    label: label("Pause"),
                    icon: Some("⏸".into()),
                    key_hint: Some("2".into()),
                    enabled: self.running,
                    is_active: false,
                    tooltip: "Pause execution".into(),
                },
                ToolbarButton::Separator,
                ToolbarButton::Action {
                    spec: None,
                    id: WidgetId::new("gallery:toolbar:filter"),
                    label: label("Filter"),
                    icon: Some("⚙".into()),
                    key_hint: Some("3".into()),
                    enabled: true,
                    is_active: self.filter_active,
                    tooltip: "Toggle filter".into(),
                },
                ToolbarButton::Action {
                    spec: None,
                    id: WidgetId::new("gallery:toolbar:debug"),
                    label: label("Debug"),
                    // Every button carries an icon so the icon-only
                    // variant has something to paint for each one.
                    icon: Some("⚑".into()),
                    key_hint: None,
                    enabled: false,
                    is_active: false,
                    tooltip: "Disabled in this build".into(),
                },
                ToolbarButton::Label {
                    text: if self.running {
                        " running".into()
                    } else {
                        " paused".into()
                    },
                    fg: Some(if self.running {
                        Color::rgb(120, 200, 120)
                    } else {
                        Color::rgb(220, 180, 80)
                    }),
                },
            ])
            .with_focused_index(self.focused_index)
    }

    /// Nerd-Font glyph + ASCII fallback pair for each action button, keyed
    /// by id — [`ToolbarIcons::apply`] bakes the right half into the
    /// returned `Toolbar` depending on `backend.nerd_fonts_enabled()`
    /// (`true` by default on GTK/macOS/Win, `false` on TUI), so a GUI host
    /// paints VS-Code-style codicons while a terminal keeps the plain
    /// Unicode glyphs declared in [`Self::toolbar`].
    fn icons() -> ToolbarIcons {
        ToolbarIcons::new()
            .with(
                WidgetId::new("gallery:toolbar:continue"),
                Icon::codicon("play", "▶").expect("\"play\" is a registered codicon name"),
            )
            .with(
                WidgetId::new("gallery:toolbar:pause"),
                Icon::codicon("debug-pause", "⏸")
                    .expect("\"debug-pause\" is a registered codicon name"),
            )
            .with(
                WidgetId::new("gallery:toolbar:filter"),
                Icon::codicon("gear", "⚙").expect("\"gear\" is a registered codicon name"),
            )
            .with(
                WidgetId::new("gallery:toolbar:debug"),
                Icon::codicon("flag", "⚑").expect("\"flag\" is a registered codicon name"),
            )
    }

    /// [`Self::toolbar`] with [`Self::icons`] resolved against `backend`'s
    /// Nerd-Font flag — the form actually painted and hit-tested.
    fn painted_toolbar(&self, variant: usize, backend: &dyn Backend) -> Toolbar {
        Self::icons().apply(&self.toolbar(variant), backend.nerd_fonts_enabled())
    }

    fn toolbar_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let h = chrome_strip_height(backend, |s| s.toolbar_height);
        Rect::new(area.x, area.y, area.width, h)
    }

    /// The hint bar below [`Self::toolbar_rect`] — a one-line
    /// [`StatusBar`], sized off `Style::status_bar_height` the same way
    /// [`crate::demos::status_bar::StatusBarDemo`] sizes its own strip.
    fn hint_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let toolbar_h = Self::toolbar_rect(area, backend).height;
        let hint_h = chrome_strip_height(backend, |s| s.status_bar_height);
        Rect::new(area.x, area.y + toolbar_h, area.width, hint_h)
    }

    fn hint_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("gallery:toolbar:hint"),
            left_segments: vec![StatusBarSegment {
                text: format!("  {} ", self.last_message),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(30, 30, 30),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }

    fn dispatch(&mut self, id: &WidgetId) {
        match id.as_str() {
            "gallery:toolbar:continue" => {
                self.running = true;
                self.last_message = "Continue".into();
            }
            "gallery:toolbar:pause" => {
                self.running = false;
                self.last_message = "Paused".into();
            }
            "gallery:toolbar:filter" => {
                self.filter_active = !self.filter_active;
                self.last_message =
                    format!("Filter {}", if self.filter_active { "on" } else { "off" });
            }
            other => {
                self.last_message = format!("Unknown action: {other}");
            }
        }
    }

    fn focusable_indices(&self, variant: usize) -> Vec<usize> {
        self.toolbar(variant)
            .buttons
            .into_iter()
            .enumerate()
            .filter_map(|(i, btn)| match btn {
                ToolbarButton::Action { enabled, .. } if enabled => Some(i),
                _ => None,
            })
            .collect()
    }

    fn advance_focus(&mut self, variant: usize, forward: bool) {
        let candidates = self.focusable_indices(variant);
        if candidates.is_empty() {
            return;
        }
        self.focused_index = Some(match self.focused_index {
            None => {
                if forward {
                    candidates[0]
                } else {
                    *candidates.last().unwrap()
                }
            }
            Some(current) => match candidates.iter().position(|&i| i == current) {
                None => candidates[0],
                Some(p) => {
                    let next = if forward {
                        (p + 1) % candidates.len()
                    } else {
                        (p + candidates.len() - 1) % candidates.len()
                    };
                    candidates[next]
                }
            },
        });
    }

    fn activate_focused(&mut self, variant: usize) -> bool {
        let Some(idx) = self.focused_index else {
            return false;
        };
        let bar = self.toolbar(variant);
        if let Some(ToolbarButton::Action { id, enabled, .. }) = bar.buttons.get(idx) {
            if *enabled {
                let id = *id;
                self.dispatch(&id);
                return true;
            }
        }
        false
    }
}

impl Default for ToolbarDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for ToolbarDemo {
    fn name(&self) -> &'static str {
        "Toolbar"
    }

    fn group(&self) -> &'static str {
        "Chrome"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Labeled", "Icon-only"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let rect = Self::toolbar_rect(area, backend);
        let bar = self.painted_toolbar(variant, backend);
        let _ = backend.draw_toolbar_interactive(rect, &bar, &self.interaction);

        let hint_rect = Self::hint_rect(area, backend);
        let _ = backend.draw_status_bar_interactive(
            hint_rect,
            &self.hint_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                ..
            } => {
                self.advance_focus(variant, true);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::BackTab),
                ..
            } => {
                self.advance_focus(variant, false);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter) | Key::Char(' '),
                ..
            } => {
                if self.activate_focused(variant) {
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
            UiEvent::KeyPressed {
                key: Key::Char(c @ '1'..='3'),
                ..
            } => {
                let id = match c {
                    '1' => "gallery:toolbar:continue",
                    '2' => "gallery:toolbar:pause",
                    _ => "gallery:toolbar:filter",
                };
                let bar = self.toolbar(variant);
                let allowed = bar.buttons.iter().any(|btn| {
                    matches!(btn, ToolbarButton::Action { id: bid, enabled, .. }
                        if bid.as_str() == id && *enabled)
                });
                if allowed {
                    self.dispatch(&WidgetId::new(id));
                    return Reaction::Redraw;
                }
                Reaction::Continue
            }
            UiEvent::MouseMoved { .. } | UiEvent::MouseDown { .. } | UiEvent::MouseUp { .. } => {
                let rect = Self::toolbar_rect(area, backend);
                let bar = self.painted_toolbar(variant, backend);
                let layout = backend.toolbar_layout(rect, &bar);
                let hit_test = |x: f32, y: f32| match layout.hit_test(x, y) {
                    ToolbarHit::Button(id) => Some(id),
                    ToolbarHit::Empty => None,
                };
                let was_pressed = self.interaction.pressed().cloned();
                let changed = self.interaction.handle_mouse(event, hit_test);

                if let UiEvent::MouseUp { position, .. } = event {
                    if let (Some(pressed_id), ToolbarHit::Button(release_id)) =
                        (was_pressed, layout.hit_test(position.x, position.y))
                    {
                        if pressed_id == release_id {
                            self.dispatch(&release_id);
                        }
                    }
                    return Reaction::Redraw;
                }
                if changed {
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        serde_json::to_value(self.toolbar(variant)).unwrap_or(serde_json::Value::Null)
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        None
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_toggles_filter() {
        let mut demo = ToolbarDemo::new();
        assert!(!demo.filter_active);
        demo.dispatch(&WidgetId::new("gallery:toolbar:filter"));
        assert!(demo.filter_active);
    }

    /// On a pixel backend (a GTK-shaped chrome line height, simulated via
    /// `RecordingBackend`), the painted toolbar rect is exactly
    /// `Style::toolbar_height` tall and the hint status bar below it is
    /// exactly `Style::status_bar_height` tall — not the raw chrome line
    /// height, the bug this test guards against regressing.
    #[test]
    fn toolbar_and_hint_rect_heights_match_their_tokens_on_a_pixel_backend() {
        let backend = quadraui::testing::RecordingBackend {
            chrome_line_height: Some(20.0),
            ..quadraui::testing::RecordingBackend::new()
        };
        let area = Rect::new(0.0, 0.0, 200.0, 100.0);
        let toolbar_rect = ToolbarDemo::toolbar_rect(area, &backend);
        assert_eq!(toolbar_rect.height, Style::default().toolbar_height);

        let hint_rect = ToolbarDemo::hint_rect(area, &backend);
        assert_eq!(hint_rect.height, Style::default().status_bar_height);
        assert_eq!(hint_rect.y, area.y + toolbar_rect.height);
    }

    #[test]
    fn advance_focus_skips_disabled_buttons() {
        let mut demo = ToolbarDemo::new();
        // `running` starts `true`, so "Continue" (index 0) is disabled —
        // the first focus stop must be "Pause" (index 1), not index 0.
        demo.advance_focus(0, true);
        assert_eq!(demo.focused_index, Some(1));
    }

    /// [`ToolbarDemo::icons`] resolves to the codicon glyph when Nerd
    /// Fonts are enabled (the GUI default) and to the demo's plain
    /// Unicode glyph otherwise (the TUI default), for every button that
    /// carries an override.
    #[test]
    fn icons_resolve_to_codicon_glyph_or_plain_unicode_fallback() {
        let demo = ToolbarDemo::new();
        let bar = demo.toolbar(0);
        let gui = ToolbarDemo::icons().apply(&bar, true);
        let tui = ToolbarDemo::icons().apply(&bar, false);

        let icon_of = |b: &Toolbar, id: &str| -> Option<String> {
            b.buttons.iter().find_map(|btn| match btn {
                ToolbarButton::Action { id: bid, icon, .. } if bid.as_str() == id => icon.clone(),
                _ => None,
            })
        };

        assert_eq!(
            icon_of(&tui, "gallery:toolbar:continue"),
            Some("▶".to_string())
        );
        let gui_continue = icon_of(&gui, "gallery:toolbar:continue").unwrap();
        assert_ne!(gui_continue, "▶");
        assert!(!gui_continue.is_empty());
    }

    #[test]
    fn icon_only_variant_leaves_no_action_button_blank() {
        let demo = ToolbarDemo::new();
        // Variant 1 drops every label, so an `icon: None` action would
        // paint as an empty gap with nothing to click or read.
        for button in &demo.toolbar(1).buttons {
            if let ToolbarButton::Action {
                id, label, icon, ..
            } = button
            {
                assert!(label.is_empty(), "{id:?} should have no label in variant 1");
                assert!(
                    icon.as_deref().is_some_and(|i| !i.is_empty()),
                    "{id:?} has neither a label nor an icon in the icon-only variant"
                );
            }
        }
        // The labelled variant keeps its labels, so the two really differ.
        assert!(demo
            .toolbar(0)
            .buttons
            .iter()
            .any(|b| matches!(b, ToolbarButton::Action { label, .. } if label == "Continue")));
    }
}

#[cfg(all(test, feature = "tui"))]
mod tui_tests {
    use super::*;

    /// TUI's fixed cell grid keeps its one-row degrade for both strips —
    /// `Style::toolbar_height`/`Style::status_bar_height` are documented
    /// no-ops there.
    #[test]
    fn toolbar_and_hint_rect_heights_stay_one_chrome_line_on_tui() {
        let backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let toolbar_rect = ToolbarDemo::toolbar_rect(area, &backend);
        assert_eq!(toolbar_rect.height, backend.chrome_line_height());

        let hint_rect = ToolbarDemo::hint_rect(area, &backend);
        assert_eq!(hint_rect.height, backend.chrome_line_height());
    }
}
