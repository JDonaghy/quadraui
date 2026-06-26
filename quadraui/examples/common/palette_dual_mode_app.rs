//! Backend-agnostic `AppLogic` for the dual-mode palette demo
//! ([`tui_palette_dual_mode`]).
//!
//! Simulates a Git branch picker with two modes:
//!
//! - **List mode** (default) — shows existing branches and lets the user
//!   fuzzy-search and select one. `Enter` "switches to" the selected branch.
//! - **Input mode** — free-text field for typing a new branch name. `Enter`
//!   "creates" the branch and switches to it.
//!
//! `Tab` (or `Shift+Tab`) toggles between modes without clearing the query.
//!
//! # Controls
//!
//! **List mode:**
//! - Type to fuzzy-filter branches (re-filter happens live; demo is
//!   client-side only — all matching is done against the static list).
//! - `↑` / `k` and `↓` / `j` — move selection.
//! - `Enter` — "switch to" the selected branch.
//! - `Tab` — switch to Input mode.
//! - `Esc` — close picker.
//!
//! **Input mode:**
//! - Type freely — the query is the new branch name.
//! - `Enter` — "create" the branch.
//! - `Tab` — switch back to List mode.
//! - `Esc` — close picker.
//!
//! **Picker closed:**
//! - `b` — reopen picker.
//! - `q` / `Esc` — quit.

use quadraui::{
    AppLogic, Backend, Color, DualModePaletteController, DualModePaletteEvent, Key, NamedKey,
    PaletteItem, PaletteMode, Reaction, Rect, StatusBar, StatusBarSegment, StyledSpan, StyledText,
    UiEvent, WidgetId, PALETTE_CHROME_ROWS,
};

/// A fixed list of pretend Git branches for the demo.
static BRANCHES: &[&str] = &[
    "main",
    "develop",
    "feature/palette-dual-mode",
    "feature/gtk-rasteriser",
    "fix/vt100-wide-char",
    "fix/word-wrap-styled",
    "refactor/backend-trait",
    "chore/deps-update",
    "ci/release-workflow",
    "docs/testing-guide",
];

pub struct PaletteDualModeApp {
    /// The controller — `None` when the picker is closed.
    picker: Option<DualModePaletteController>,
    /// Most recently selected/created branch name.
    current_branch: String,
    /// Status message shown in the status bar.
    status: String,
}

impl PaletteDualModeApp {
    pub fn new() -> Self {
        let picker = make_picker(BRANCHES);
        Self {
            picker: Some(picker),
            current_branch: "main".into(),
            status: "Branch picker  Tab=toggle mode  Enter=confirm  Esc=close".into(),
        }
    }

    fn popup_rect(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        let w = (vp.width * 0.55).max(50.0);
        let h = (vp.height * 0.55).max(12.0 * backend.line_height());
        let x = (vp.width - w) / 2.0;
        let y = (vp.height - h) / 2.0;
        Rect::new(x, y, w, h)
    }

    fn status_bar(&self) -> StatusBar {
        let mode_label = match &self.picker {
            Some(p) => match p.mode() {
                PaletteMode::List => " [LIST] ",
                PaletteMode::Input => " [INPUT] ",
            },
            None => " [closed] ",
        };
        StatusBar {
            id: WidgetId::new("status"),
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
                    text: format!("  {}", self.current_branch),
                    fg: Color::rgb(180, 255, 140),
                    bg: Color::rgb(30, 80, 30),
                    bold: false,
                    action_id: None,
                },
            ],
        }
    }
}

impl Default for PaletteDualModeApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for PaletteDualModeApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let lh = backend.line_height();

        // Status bar.
        let bar_h = lh * 1.5;
        let bar_rect = Rect::new(0.0, vp.height - bar_h, vp.width, bar_h);
        backend.draw_status_bar(bar_rect, &self.status_bar(), None, None);

        // Dual-mode palette (when open).
        if let Some(ref picker) = self.picker {
            let popup_rect = Self::popup_rect(backend);
            picker.render(popup_rect, backend);
        }
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        if let Some(ref mut picker) = self.picker {
            let popup_rect = Self::popup_rect(backend);
            let lh = backend.line_height();
            let popup_h_rows = if lh > 0.0 {
                (popup_rect.height / lh) as usize
            } else {
                20
            };
            let visible_rows = popup_h_rows.saturating_sub(PALETTE_CHROME_ROWS);

            let ev = picker.handle(&event, visible_rows);
            match ev {
                DualModePaletteEvent::ItemConfirmed { idx } => {
                    // Recompute filtered list from current query to map idx
                    // back to a branch name.
                    let query = picker.query().to_lowercase();
                    let matched: Vec<&&str> = BRANCHES
                        .iter()
                        .filter(|b| b.to_lowercase().contains(&query))
                        .collect();
                    if let Some(branch) = matched.get(idx) {
                        self.current_branch = (*branch).to_string();
                        self.status = format!(
                            "Switched to '{}'  —  press 'b' to reopen, q/Esc to quit",
                            self.current_branch
                        );
                    }
                    self.picker = None;
                    return Reaction::Redraw;
                }
                DualModePaletteEvent::TextConfirmed { value } => {
                    if !value.is_empty() {
                        self.current_branch = value.clone();
                        self.status = format!(
                            "Created and switched to '{}'  —  press 'b' to reopen",
                            value
                        );
                    }
                    self.picker = None;
                    return Reaction::Redraw;
                }
                DualModePaletteEvent::QueryChanged { value } => {
                    // Refilter the branch list for List mode.
                    let q = value.to_lowercase();
                    let matching: Vec<&str> = BRANCHES
                        .iter()
                        .copied()
                        .filter(|b| b.to_lowercase().contains(&q))
                        .collect();
                    picker.set_items(branches_as_items(&matching));
                    return Reaction::Redraw;
                }
                DualModePaletteEvent::ModeToggled { new_mode } => {
                    self.status = match new_mode {
                        PaletteMode::Input => {
                            "Input mode: type a new branch name, Enter to create, Tab to switch"
                                .into()
                        }
                        PaletteMode::List => {
                            "List mode: type to filter, Enter to select, Tab to switch".into()
                        }
                    };
                    // Restore the full item list when switching back to List mode.
                    if new_mode == PaletteMode::List {
                        let q = picker.query().to_lowercase();
                        let matching: Vec<&str> = BRANCHES
                            .iter()
                            .copied()
                            .filter(|b| b.to_lowercase().contains(&q))
                            .collect();
                        picker.set_items(branches_as_items(&matching));
                    }
                    return Reaction::Redraw;
                }
                DualModePaletteEvent::Cancelled => {
                    self.status = "Picker closed  —  press 'b' to reopen, q/Esc to quit".into();
                    self.picker = None;
                    return Reaction::Redraw;
                }
                DualModePaletteEvent::Consumed => return Reaction::Redraw,
                DualModePaletteEvent::Ignored => {}
            }
        } else {
            // Picker is closed — handle reopen / quit.
            if let UiEvent::KeyPressed { ref key, .. } = event {
                match key {
                    Key::Char('q') | Key::Named(NamedKey::Escape) => return Reaction::Exit,
                    Key::Char('b') => {
                        self.picker = Some(make_picker(BRANCHES));
                        self.status =
                            "Branch picker  Tab=toggle mode  Enter=confirm  Esc=close".into();
                        return Reaction::Redraw;
                    }
                    _ => {}
                }
            }
        }

        match event {
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_picker(branches: &[&str]) -> DualModePaletteController {
    let items = branches_as_items(branches);
    DualModePaletteController::new("Switch Branch", Some("New branch:".into()), items)
        .with_id("branch_picker")
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
