//! `Command Palette (dual mode)` demo — adapted from
//! `quadraui/examples/common/palette_dual_mode_app.rs`.
//!
//! A pretend Git branch picker: **List mode** fuzzy-filters existing
//! branches; `Tab` switches to **Input mode**, a free-text field for
//! naming a new one. Two variants show the popup at two sizes.
//!
//! The gallery hands each demo raw events, so the demo drives its
//! [`DualModePalette`] with [`drive_ui_event`] and applies what it reports.

use quadraui::compose::{DualModePaletteAction, Handled};
use quadraui::DrawsPrimitives;
use quadraui::{
    drive_ui_event, Backend, BackendCaps, Color, DualModePalette, DualModePaletteState,
    MouseButton, PaletteItem, PaletteMode, Reaction, Rect, StatusBar, StatusBarSegment, Style,
    StyledSpan, StyledText, UiEvent, WidgetId, PALETTE_CHROME_ROWS,
};

use crate::demo::{extract_region, CaptureAction, Demo};

const SOURCE: &str = include_str!("palette.rs");

/// `Style`'s chrome-metric `token`, floored at the chrome line height on
/// a pixel backend — never shrunk below it, so a chrome font taller than
/// the design token isn't clipped — or the plain chrome line height on
/// TUI's fixed cell grid, where every `Style` chrome metric is a
/// documented no-op. Mirrors `ShellView`'s own token-vs-line-height carve
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
static BRANCHES: &[&str] = &[
    "main",
    "develop",
    "feature/palette-dual-mode",
    "feature/gtk-rasteriser",
    "fix/vt100-wide-char",
    "fix/word-wrap-styled",
    "refactor/backend-trait",
    "chore/deps-update",
];

pub struct PaletteDemo {
    picker: DualModePalette,
    /// The state the picker routes through: its live query caret.
    picker_state: DualModePaletteState,
    current_branch: String,
    status: String,
}

impl PaletteDemo {
    pub fn new() -> Self {
        Self {
            picker: make_picker(BRANCHES),
            picker_state: DualModePaletteState::default(),
            current_branch: "main".into(),
            status: "Tab=toggle mode  Enter=confirm".into(),
        }
    }

    fn popup_rect(variant: usize, area: Rect, backend: &dyn Backend) -> Rect {
        let scale = if variant == 1 { 0.9 } else { 0.55 };
        let w = (area.width * scale).max(40.0);
        let h = (area.height * 0.6).max(10.0 * backend.line_height());
        let x = area.x + (area.width - w) / 2.0;
        let y = area.y + (area.height - h) / 2.0;
        Rect::new(x, y, w, h)
    }

    fn status_bar(&self) -> StatusBar {
        let mode_label = match self.picker.mode {
            PaletteMode::List => " [LIST] ",
            PaletteMode::Input => " [INPUT] ",
        };
        StatusBar {
            id: WidgetId::new("gallery:palette:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.status),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(35, 55, 90),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![
                StatusBarSegment {
                    text: mode_label.into(),
                    fg: Color::rgb(100, 200, 255),
                    bg: Color::rgb(30, 50, 80),
                    bold: true,
                    action_id: None,
                },
                StatusBarSegment {
                    text: format!("  {}  ", self.current_branch),
                    fg: Color::rgb(180, 255, 140),
                    bg: Color::rgb(30, 80, 30),
                    bold: false,
                    action_id: None,
                },
            ],
        }
    }
}

impl Default for PaletteDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for PaletteDemo {
    fn name(&self) -> &'static str {
        "Command Palette"
    }

    fn group(&self) -> &'static str {
        "Content"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Compact", "Wide"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let bar_h = chrome_strip_height(backend, |s| s.status_bar_height);
        let bar_rect = Rect::new(area.x, area.y + area.height - bar_h, area.width, bar_h);
        let _ = backend.draw_dyn(bar_rect, &self.status_bar(), &());

        // The picker's state lives on this demo, not in the runtime's
        // store, so it is lent to the store for the paint, which draws the
        // caret from it.
        let popup_rect = Self::popup_rect(variant, area, backend);
        let id = self.picker.id;
        quadraui::with_widget_states(|states| {
            states.insert(id, Box::new(self.picker_state.clone()))
        });
        let _ = backend.draw_dyn(popup_rect, &self.picker, &());
        quadraui::with_widget_states(|states| states.remove(&id));
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        let popup_rect = Self::popup_rect(variant, area, backend);
        let lh = backend.line_height();
        let popup_rows = if lh > 0.0 {
            (popup_rect.height / lh) as usize
        } else {
            20
        };
        let visible_rows = popup_rows.saturating_sub(PALETTE_CHROME_ROWS);

        // Keys always reach the picker, a paste only while it takes free
        // text, and a left press or a move only inside the popup.
        let routed = match event {
            UiEvent::KeyPressed { .. } => true,
            UiEvent::ClipboardPaste(_) => self.picker.mode == PaletteMode::Input,
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            }
            | UiEvent::MouseMoved { position, .. } => popup_rect.contains(*position),
            _ => false,
        };
        let ev = if routed {
            drive_ui_event(
                &self.picker,
                &mut self.picker_state,
                event,
                backend,
                popup_rect,
            )
            .map_or(Handled::Ignored, |driven| driven.handled)
        } else {
            Handled::Ignored
        };
        if let Handled::Event(action) = &ev {
            self.picker.apply(action, visible_rows);
        }

        match ev {
            Handled::Event(DualModePaletteAction::ItemConfirmed { idx }) => {
                let query = self.picker.query.to_lowercase();
                let matched: Vec<&&str> = BRANCHES
                    .iter()
                    .filter(|b| b.to_lowercase().contains(&query))
                    .collect();
                if let Some(branch) = matched.get(idx) {
                    self.current_branch = (*branch).to_string();
                    self.status = format!("Switched to '{}'", self.current_branch);
                }
                Reaction::Redraw
            }
            Handled::Event(DualModePaletteAction::TextConfirmed { value }) => {
                if !value.is_empty() {
                    self.current_branch = value.clone();
                    self.status = format!("Created and switched to '{value}'");
                }
                Reaction::Redraw
            }
            Handled::Event(DualModePaletteAction::QueryChanged { value }) => {
                let q = value.to_lowercase();
                let matching: Vec<&str> = BRANCHES
                    .iter()
                    .copied()
                    .filter(|b| b.to_lowercase().contains(&q))
                    .collect();
                self.picker.set_items(branches_as_items(&matching));
                Reaction::Redraw
            }
            Handled::Event(DualModePaletteAction::ModeToggled { new_mode }) => {
                self.status = match new_mode {
                    PaletteMode::Input => "Type a new branch name, Enter to create".into(),
                    PaletteMode::List => "Type to filter, Enter to select".into(),
                };
                if new_mode == PaletteMode::List {
                    let q = self.picker.query.to_lowercase();
                    let matching: Vec<&str> = BRANCHES
                        .iter()
                        .copied()
                        .filter(|b| b.to_lowercase().contains(&q))
                        .collect();
                    self.picker.set_items(branches_as_items(&matching));
                }
                Reaction::Redraw
            }
            Handled::Event(DualModePaletteAction::Cancelled) => {
                self.status = "Dismissed — reopen by switching variants".into();
                Reaction::Redraw
            }
            Handled::Event(_) | Handled::Consumed => Reaction::Redraw,
            // Escape falls through to `Reaction::Continue` — the
            // gallery shell quits on an unconsumed Escape, matching
            // every other gallery demo's convention of not re-handling
            // the quit key itself.
            Handled::Ignored => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, _variant: usize) -> serde_json::Value {
        serde_json::json!({
            "mode": format!("{:?}", self.picker.mode),
            "query": self.picker.query.as_str(),
            "current_branch": self.current_branch,
            "status": self.status,
        })
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        None
    }

    fn hover_action(&self, _variant: usize) -> Option<CaptureAction> {
        // A pointer move over a row moves the highlighted row
        // (`selected`) the same way a click does: this palette has no
        // hover channel distinct from `selected`. A real `MouseMoved` over
        // "develop" (not the default `selected == 0` "main" row) is
        // enough to tell this capture apart from the idle "rest" one.
        Some(CaptureAction::HoverText("develop"))
    }
}

fn make_picker(branches: &[&str]) -> DualModePalette {
    DualModePalette::new(
        WidgetId::new("gallery:branch-picker"),
        "Switch Branch",
        Some("New branch:".into()),
        branches_as_items(branches),
    )
    .with_vim_keys(true)
}

fn branches_as_items(branches: &[&str]) -> Vec<PaletteItem> {
    branches
        .iter()
        .map(|name| PaletteItem {
            text: StyledText {
                spans: vec![StyledSpan::plain(*name)],
            },
            detail: None,
            icon: None,
            match_positions: vec![],
            depth: 0,
            expandable: false,
            expanded: false,
        })
        .collect()
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_is_wider_in_the_wide_variant() {
        let area = Rect::new(0.0, 0.0, 100.0, 40.0);
        let compact = PaletteDemo::popup_rect(0, area, &quadraui::testing::RecordingBackend::new());
        let wide = PaletteDemo::popup_rect(1, area, &quadraui::testing::RecordingBackend::new());
        assert!(wide.width > compact.width);
    }

    #[test]
    fn new_starts_on_main_branch_in_list_mode() {
        let demo = PaletteDemo::new();
        assert_eq!(demo.current_branch, "main");
        assert_eq!(demo.picker.mode, PaletteMode::List);
    }
}
