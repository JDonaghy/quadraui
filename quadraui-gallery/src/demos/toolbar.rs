//! `Toolbar` demo — adapted from `quadraui/examples/common/toolbar_app.rs`.
//!
//! Exercises action buttons, a toggle action, a permanently-disabled
//! action, a separator, a non-clickable label, hover/press state and
//! keyboard focus (Tab / Shift-Tab / Enter) — the `Toolbar` primitive's
//! full surface.

use quadraui::{
    Backend, BackendCaps, Color, Icon, InteractionState, Key, NamedKey, Reaction, Rect, StatusBar,
    StatusBarSegment, Style, Toolbar, ToolbarButton, ToolbarHit, ToolbarIcons, ToolbarLayout,
    ToolbarLayoutOptions, ToolbarOverflow, ToolbarOverflowPriorities, UiEvent, WidgetId,
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

/// The view-mode segmented control's four options (id, label, icon), in
/// display order — shared by [`ToolbarDemo::toolbar`] (which builds the
/// matching `ToolbarButton::Group` children) and [`ToolbarDemo::dispatch`]
/// (which reads a `GroupSelected`/`ButtonClicked` id back into one of
/// these). Every option carries its own icon glyph so the icon-only
/// variant still has something to paint for each one — same rule
/// [`ToolbarDemo::toolbar`]'s flat actions already follow.
const VIEW_MODES: [(&str, &str, &str); 4] = [
    ("icons", "Icons", "▦"),
    ("list", "List", "☰"),
    ("columns", "Columns", "⬚"),
    ("gallery", "Gallery", "▣"),
];

// gallery:begin
pub struct ToolbarDemo {
    filter_active: bool,
    running: bool,
    last_message: String,
    interaction: InteractionState,
    focused_index: Option<usize>,
    /// Which [`VIEW_MODES`] option is active — the exclusive 4-way
    /// segmented group's own selection state, owned by the host the same
    /// way `filter_active` owns the plain toggle action's.
    view_mode: &'static str,
}

impl ToolbarDemo {
    pub fn new() -> Self {
        Self {
            filter_active: false,
            running: true,
            last_message: "Click, Tab to focus, Enter to activate".into(),
            interaction: InteractionState::new(),
            focused_index: None,
            view_mode: "icons",
        }
    }

    /// `variant == 1` ("Icon-only") drops button labels and keeps only
    /// icon + tooltip — the compact style a space-constrained host
    /// toolbar would use. Every other variant, including the narrow-bar
    /// `Overflow: Wrap`/`Overflow: Menu` ones, keeps labels: those exist
    /// to show *how the same labeled bar degrades* in a narrower slot,
    /// not to also exercise the icon-only style at once.
    fn toolbar(&self, variant: usize) -> Toolbar {
        let labeled = variant != 1;
        let label = |text: &str| if labeled { text.into() } else { String::new() };
        Toolbar::new(WidgetId::new("gallery:toolbar"))
            .with_buttons(vec![
                // A non-exclusive group: back/forward share one bordered
                // pill, but each clicks through as its own independent
                // `ButtonClicked` — neither is a toggle on the other.
                ToolbarButton::Group {
                    id: WidgetId::new("gallery:toolbar:nav"),
                    exclusive: false,
                    label: Some("History navigation".into()),
                    items: vec![
                        ToolbarButton::Action {
                            spec: None,
                            id: WidgetId::new("gallery:toolbar:back"),
                            label: label("Back"),
                            icon: Some("←".into()),
                            key_hint: None,
                            enabled: true,
                            is_active: false,
                            tooltip: "Go back".into(),
                        },
                        ToolbarButton::Action {
                            spec: None,
                            id: WidgetId::new("gallery:toolbar:forward"),
                            label: label("Forward"),
                            icon: Some("→".into()),
                            key_hint: None,
                            enabled: true,
                            is_active: false,
                            tooltip: "Go forward".into(),
                        },
                    ],
                },
                // An exclusive group: a view-mode segmented control where
                // exactly one of the four options is ever `is_active`.
                ToolbarButton::Group {
                    id: WidgetId::new("gallery:toolbar:view"),
                    exclusive: true,
                    label: Some("View mode".into()),
                    items: VIEW_MODES
                        .iter()
                        .map(|(id, text, icon)| ToolbarButton::Action {
                            spec: None,
                            id: WidgetId::new(format!("gallery:toolbar:view:{id}")),
                            label: label(text),
                            icon: Some((*icon).into()),
                            key_hint: None,
                            enabled: true,
                            is_active: self.view_mode == *id,
                            tooltip: format!("Switch to {text} view"),
                        })
                        .collect(),
                },
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

    /// Variant 2 ("Overflow: Wrap") / 3 ("Overflow: Menu")'s narrow-bar
    /// twin of [`Self::toolbar_rect`] — deliberately narrower than the
    /// full labeled toolbar's combined button width so `mode` always has
    /// something to show, the same "pick a bar narrower than its
    /// buttons" setup `quadraui/examples/common/toolbar_overflow_demo.rs`
    /// uses for its own fixed-width slot.
    fn overflow_rect(area: Rect, backend: &dyn Backend, mode: ToolbarOverflow) -> Rect {
        let h = chrome_strip_height(backend, |s| s.toolbar_height);
        let width = (area.width * 0.4).max(h * 12.0).min(area.width);
        // `Wrap` needs enough rows for every top-level item (both groups
        // plus the three flat actions) to land on its own line at
        // `width` — sized generously up front the same way
        // `toolbar_overflow_demo.rs`'s `BAR_ROWS` is, rather than
        // following `ToolbarLayout::cross_extent`'s two-pass protocol.
        // `Menu` never grows past one line: dropped items move behind
        // the trailing `»` affordance instead of onto a new row.
        let rows = match mode {
            ToolbarOverflow::Wrap => 6.0,
            _ => 1.0,
        };
        Rect::new(area.x, area.y, width, h * rows)
    }

    /// The hint bar below an [`Self::overflow_rect`] slot — positioned
    /// off that rect's own painted height rather than
    /// [`Self::hint_rect`]'s single-chrome-line assumption, since `Wrap`
    /// reserves several rows.
    fn overflow_hint_rect(area: Rect, bar_rect: Rect, backend: &dyn Backend) -> Rect {
        let hint_h = chrome_strip_height(backend, |s| s.status_bar_height);
        Rect::new(area.x, bar_rect.y + bar_rect.height, area.width, hint_h)
    }

    /// `variant`'s [`ToolbarOverflow`] mode, or `None` for the two plain
    /// `Clip` variants ("Labeled", "Icon-only").
    fn overflow_mode(variant: usize) -> Option<ToolbarOverflow> {
        match variant {
            2 => Some(ToolbarOverflow::Wrap),
            3 => Some(ToolbarOverflow::Menu),
            _ => None,
        }
    }

    fn overflow_options(mode: ToolbarOverflow) -> ToolbarLayoutOptions {
        ToolbarLayoutOptions::new().with_overflow(mode)
    }

    /// Drop priorities for [`ToolbarOverflow::Menu`] — lowest dropped
    /// first. The permanently-disabled "Debug" button goes first since
    /// losing it costs nothing; the exclusive view-mode group goes last
    /// since it is the one a caller is least likely to want hidden.
    fn overflow_priorities() -> ToolbarOverflowPriorities {
        ToolbarOverflowPriorities::new()
            .with(WidgetId::new("gallery:toolbar:debug"), 0)
            .with(WidgetId::new("gallery:toolbar:filter"), 1)
            .with(WidgetId::new("gallery:toolbar:pause"), 2)
            .with(WidgetId::new("gallery:toolbar:continue"), 3)
            .with(WidgetId::new("gallery:toolbar:nav"), 4)
            .with(WidgetId::new("gallery:toolbar:view"), 5)
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
            "gallery:toolbar:back" => {
                self.last_message = "Back".into();
            }
            "gallery:toolbar:forward" => {
                self.last_message = "Forward".into();
            }
            other => {
                if let Some(stripped) = other.strip_prefix("gallery:toolbar:view:") {
                    if let Some((id, text, _)) = VIEW_MODES.iter().find(|(id, ..)| *id == stripped)
                    {
                        self.view_mode = id;
                        self.last_message = format!("View: {text}");
                        return;
                    }
                }
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
        &["Labeled", "Icon-only", "Overflow: Wrap", "Overflow: Menu"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        match Self::overflow_mode(variant) {
            None => {
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
            Some(mode) => {
                let rect = Self::overflow_rect(area, backend, mode);
                let bar = self.painted_toolbar(variant, backend);
                let _ = backend.draw_toolbar_with_overflow(
                    rect,
                    &bar,
                    &self.interaction,
                    Self::overflow_options(mode),
                    &Self::overflow_priorities(),
                );

                let hint_rect = Self::overflow_hint_rect(area, rect, backend);
                let _ = backend.draw_status_bar_interactive(
                    hint_rect,
                    &self.hint_bar(),
                    &InteractionState::new(),
                );
            }
        }
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
                let mode = Self::overflow_mode(variant);
                let rect = match mode {
                    Some(m) => Self::overflow_rect(area, backend, m),
                    None => Self::toolbar_rect(area, backend),
                };
                let bar = self.painted_toolbar(variant, backend);
                let layout: ToolbarLayout = match mode {
                    Some(m) => backend.toolbar_layout_with_overflow(
                        rect,
                        &bar,
                        Self::overflow_options(m),
                        &Self::overflow_priorities(),
                    ),
                    // `Clip`'s plain layout — `Overflow` never fires here,
                    // so it reads the same as `Empty` if it ever does.
                    None => backend.toolbar_layout(rect, &bar),
                };
                let hit_test = |x: f32, y: f32| match layout.hit_test(x, y) {
                    ToolbarHit::Button(id) => Some(id),
                    // `Overflow` is handled in the `MouseUp` arm below,
                    // not through `InteractionState`'s hover/press
                    // tracking.
                    _ => None,
                };
                let was_pressed = self.interaction.pressed().cloned();
                let changed = self.interaction.handle_mouse(event, hit_test);

                if let UiEvent::MouseUp { position, .. } = event {
                    match layout.hit_test(position.x, position.y) {
                        ToolbarHit::Button(release_id) if was_pressed == Some(release_id) => {
                            self.dispatch(&release_id);
                        }
                        ToolbarHit::Overflow => {
                            let dropped = layout.overflowed_items();
                            let names: Vec<String> = dropped
                                .iter()
                                .map(|item| {
                                    item.label
                                        .spans
                                        .iter()
                                        .map(|s| s.text.as_str())
                                        .collect::<String>()
                                })
                                .collect();
                            self.last_message = if names.is_empty() {
                                "Nothing overflowed".into()
                            } else {
                                format!("Overflowed: {}", names.join(", "))
                            };
                        }
                        _ => {}
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

    // No `capture_action` override: both overflow variants' idle render
    // already shows the mode's whole point — `Overflow: Wrap`'s extra
    // rows, `Overflow: Menu`'s trailing `»` affordance — without needing
    // a click first. `CaptureAction::ClickText` also wouldn't reach
    // `ToolbarHit::Overflow` here: it only delivers a `MouseDown`
    // ([`quadraui::tui::testing::TuiDriver::click`]'s own doc), and this
    // demo's clicks dispatch on `MouseUp` (the same press-then-release-
    // on-the-same-target convention every button here follows).
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
        // The two leading `Group` buttons (nav, view) aren't `Action`s,
        // so `focusable_indices` skips straight over them; `running`
        // starts `true`, so "Continue" right after them is also
        // disabled — the first focus stop must be "Pause".
        demo.advance_focus(0, true);
        let continue_idx = demo
            .toolbar(0)
            .buttons
            .iter()
            .position(|b| matches!(b, ToolbarButton::Action { id, .. } if id.as_str() == "gallery:toolbar:continue"))
            .unwrap();
        assert_eq!(demo.focused_index, Some(continue_idx + 1));
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

    /// Variant 2 ("Overflow: Wrap")'s narrow slot is actually narrower
    /// than the full labeled toolbar's natural width, so its items
    /// really do spread across more than one row — not a `Clip` layout
    /// that happened to still fit on one.
    #[test]
    fn overflow_wrap_variant_spreads_items_across_more_than_one_row() {
        let backend = quadraui::tui::TuiBackend::new();
        let demo = ToolbarDemo::new();
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let mode = ToolbarDemo::overflow_mode(2).expect("variant 2 is Wrap");
        let rect = ToolbarDemo::overflow_rect(area, &backend, mode);
        let bar = demo.painted_toolbar(2, &backend);
        let layout = backend.toolbar_layout_with_overflow(
            rect,
            &bar,
            ToolbarDemo::overflow_options(mode),
            &ToolbarDemo::overflow_priorities(),
        );
        let rows: std::collections::BTreeSet<i32> = layout
            .visible_items
            .iter()
            .map(|vis| vis.bounds.y.round() as i32)
            .collect();
        assert!(
            rows.len() > 1,
            "Wrap items should land on more than one row at this width, got rows {rows:?}"
        );
    }

    /// Variant 3 ("Overflow: Menu") drops at least one top-level item
    /// behind the trailing `»` affordance at the same narrow width.
    #[test]
    fn overflow_menu_variant_drops_at_least_one_item() {
        let backend = quadraui::tui::TuiBackend::new();
        let demo = ToolbarDemo::new();
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let mode = ToolbarDemo::overflow_mode(3).expect("variant 3 is Menu");
        let rect = ToolbarDemo::overflow_rect(area, &backend, mode);
        let bar = demo.painted_toolbar(3, &backend);
        let layout = backend.toolbar_layout_with_overflow(
            rect,
            &bar,
            ToolbarDemo::overflow_options(mode),
            &ToolbarDemo::overflow_priorities(),
        );
        assert!(
            !layout.overflowed_items().is_empty(),
            "Menu mode should drop at least one top-level item at this width"
        );
    }

    /// Clicking the `»` affordance drives [`ToolbarDemo::handle`]'s
    /// `ToolbarHit::Overflow` arm end to end: it reports the dropped
    /// items in the hint line, the same contract
    /// `quadraui/examples/common/toolbar_overflow_demo.rs`'s own
    /// status-line update follows.
    #[test]
    fn clicking_the_overflow_affordance_reports_dropped_items_in_the_hint_line() {
        let mut backend = quadraui::tui::TuiBackend::new();
        let mut demo = ToolbarDemo::new();
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);

        let mode = ToolbarDemo::overflow_mode(3).unwrap();
        let rect = ToolbarDemo::overflow_rect(area, &backend, mode);
        let bar = demo.painted_toolbar(3, &backend);
        let layout = backend.toolbar_layout_with_overflow(
            rect,
            &bar,
            ToolbarDemo::overflow_options(mode),
            &ToolbarDemo::overflow_priorities(),
        );
        let overflow_vis = layout
            .visible_items
            .iter()
            .find(|vis| vis.kind == quadraui::ToolbarItemKind::Overflow)
            .expect("Menu mode should paint the » affordance at this width");
        let x = overflow_vis.bounds.x + overflow_vis.bounds.width / 2.0;
        let y = overflow_vis.bounds.y + overflow_vis.bounds.height / 2.0;

        demo.handle(
            3,
            &quadraui::mouse_down(quadraui::MouseButton::Left, x, y, Default::default()),
            &mut backend,
            area,
        );
        demo.handle(
            3,
            &quadraui::mouse_up(quadraui::MouseButton::Left, x, y),
            &mut backend,
            area,
        );

        assert!(
            demo.last_message.starts_with("Overflowed:"),
            "clicking » should report dropped items, got {:?}",
            demo.last_message
        );
    }
}
