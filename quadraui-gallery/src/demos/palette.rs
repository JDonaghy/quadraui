//! `Command Palette (dual mode)` demo — adapted from
//! `quadraui/examples/common/palette_dual_mode_app.rs`.
//!
//! A pretend Git branch picker: **List mode** fuzzy-filters existing
//! branches; `Tab` switches to **Input mode**, a free-text field for
//! naming a new one. Two variants show the popup at two sizes.

use quadraui::{
    Backend, BackendCaps, Color, DualModePaletteController, DualModePaletteEvent, InteractionState,
    PaletteItem, PaletteMode, Reaction, Rect, StatusBar, StatusBarSegment, StyledSpan, StyledText,
    UiEvent, WidgetId, PALETTE_CHROME_ROWS,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("palette.rs");

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
    picker: DualModePaletteController,
    current_branch: String,
    status: String,
}

impl PaletteDemo {
    pub fn new() -> Self {
        Self {
            picker: make_picker(BRANCHES),
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
        let mode_label = match self.picker.mode() {
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
        let lh = backend.line_height();
        let bar_h = lh * 1.5;
        let bar_rect = Rect::new(area.x, area.y + area.height - bar_h, area.width, bar_h);
        let _ = backend.draw_status_bar_interactive(
            bar_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );

        let popup_rect = Self::popup_rect(variant, area, backend);
        self.picker.render(popup_rect, backend);
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

        let ev = match event {
            UiEvent::MouseDown { .. } => {
                self.picker
                    .handle_mouse(event, backend, popup_rect, visible_rows)
            }
            _ => self.picker.handle(event, visible_rows),
        };

        match ev {
            DualModePaletteEvent::ItemConfirmed { idx } => {
                let query = self.picker.query().to_lowercase();
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
            DualModePaletteEvent::TextConfirmed { value } => {
                if !value.is_empty() {
                    self.current_branch = value.clone();
                    self.status = format!("Created and switched to '{value}'");
                }
                Reaction::Redraw
            }
            DualModePaletteEvent::QueryChanged { value } => {
                let q = value.to_lowercase();
                let matching: Vec<&str> = BRANCHES
                    .iter()
                    .copied()
                    .filter(|b| b.to_lowercase().contains(&q))
                    .collect();
                self.picker.set_items(branches_as_items(&matching));
                Reaction::Redraw
            }
            DualModePaletteEvent::ModeToggled { new_mode } => {
                self.status = match new_mode {
                    PaletteMode::Input => "Type a new branch name, Enter to create".into(),
                    PaletteMode::List => "Type to filter, Enter to select".into(),
                };
                if new_mode == PaletteMode::List {
                    let q = self.picker.query().to_lowercase();
                    let matching: Vec<&str> = BRANCHES
                        .iter()
                        .copied()
                        .filter(|b| b.to_lowercase().contains(&q))
                        .collect();
                    self.picker.set_items(branches_as_items(&matching));
                }
                Reaction::Redraw
            }
            DualModePaletteEvent::Cancelled => {
                self.status = "Dismissed — reopen by switching variants".into();
                Reaction::Redraw
            }
            DualModePaletteEvent::Consumed => Reaction::Redraw,
            // Escape falls through to `Reaction::Continue` — the
            // gallery shell quits on an unconsumed Escape, matching
            // every other gallery demo's convention of not re-handling
            // the quit key itself.
            DualModePaletteEvent::Ignored => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, _variant: usize) -> serde_json::Value {
        serde_json::json!({
            "mode": format!("{:?}", self.picker.mode()),
            "query": self.picker.query(),
            "current_branch": self.current_branch,
            "status": self.status,
        })
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        None
    }
}

fn make_picker(branches: &[&str]) -> DualModePaletteController {
    let items = branches_as_items(branches);
    DualModePaletteController::new("Switch Branch", Some("New branch:".into()), items)
        .with_id("gallery:branch-picker")
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
        assert_eq!(demo.picker.mode(), PaletteMode::List);
    }
}
