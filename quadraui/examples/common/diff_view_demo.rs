//! `DiffViewApp` — demonstrates `compute_hunks` + `DiffView` + scroll.
//!
//! Shows a side-by-side diff of two small Rust functions with a few
//! additions, removals, and changed lines.
//!
//! ## Key bindings
//!
//! | Key            | Action                        |
//! |----------------|-------------------------------|
//! | `j` / Down     | Scroll down one row           |
//! | `k` / Up       | Scroll up one row             |
//! | Page Down      | Scroll down by visible height |
//! | Page Up        | Scroll up by visible height   |
//! | `m`            | Toggle SideBySide ↔ Unified   |
//! | `q` / Esc      | Quit                          |

use std::cell::Cell;

use quadraui::backend::Backend;
use quadraui::diff::compute_hunks;
use quadraui::event::{Key, MouseButton, NamedKey, Rect, UiEvent};
use quadraui::primitives::diff_view::{
    DiffEditability, DiffMode, DiffPane, DiffView, DiffViewHit, DiffViewLayout,
};
use quadraui::runner::{AppLogic, Reaction};
use quadraui::types::{Color, WidgetId};
use quadraui::{InteractionState, StatusBar, StatusBarSegment};

const LEFT: &str = "\
fn add(a: i32, b: i32) -> i32 {
    a + b
}

fn multiply(a: i32, b: i32) -> i32 {
    a * b
}

fn main() {
    println!(\"{}\", add(3, 4));
}";

const RIGHT: &str = "\
fn add(a: i32, b: i32) -> i64 {
    (a + b) as i64
}

fn multiply(a: i32, b: i32) -> i32 {
    a * b
}

fn subtract(a: i32, b: i32) -> i32 {
    a - b
}

fn main() {
    println!(\"{}\", add(3, 4));
    println!(\"{}\", subtract(10, 3));
}";

/// Application demonstrating the `DiffView` primitive.
pub struct DiffViewApp {
    view: DiffView,
    /// Cached layout from the last rendered frame, used for scroll clamping.
    /// Wrapped in `Cell` so `render(&self, ...)` can update it.
    last_layout: Cell<DiffViewLayout>,
    /// Human-readable description of the last click, shown in the status
    /// bar (#818 — proves `DiffViewGeometry::hit_test` reaches a real
    /// click, not just a unit test against the primitive in isolation).
    last_click: Option<String>,
}

impl DiffViewApp {
    /// Construct a `DiffViewApp` with pre-computed hunks from the demo content.
    pub fn new() -> Self {
        let hunks = compute_hunks(LEFT, RIGHT);
        let view = DiffView {
            id: WidgetId::new("diff-view-demo"),
            left: LEFT.to_string(),
            right: RIGHT.to_string(),
            left_label: Some("original".to_string()),
            right_label: Some("modified".to_string()),
            hunks,
            mode: DiffMode::SideBySide,
            editability: DiffEditability::ReadOnly,
            scroll_offset: 0,
            focused_pane: DiffPane::Left,
            has_focus: true,
        };
        Self {
            view,
            last_layout: Cell::new(DiffViewLayout {
                visible_rows: 24,
                total_rows: 0,
            }),
            last_click: None,
        }
    }

    /// Diff-view rect (viewport minus the 1-row status bar at the
    /// bottom). Shared by `render` and `handle` so paint and
    /// click-routing can't disagree (same rule the other demos in this
    /// module follow — see `text_input_demo::TextInputDemo::input_rect`).
    fn diff_rect(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        let lh = backend.line_height();
        Rect::new(0.0, 0.0, vp.width, (vp.height - lh).max(0.0))
    }

    fn status_bar(&self) -> StatusBar {
        let msg = match &self.last_click {
            Some(m) => format!(" {m} "),
            None => " click a row — j/k scroll, m toggles mode, q quits ".into(),
        };
        StatusBar {
            id: WidgetId::new("diff-view-status"),
            left_segments: vec![StatusBarSegment {
                text: msg,
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }
}

impl Default for DiffViewApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for DiffViewApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let rect = Self::diff_rect(backend);
        let layout = backend.draw_diff_view(rect, &self.view);
        self.last_layout.set(layout);

        let status_rect = Rect::new(0.0, rect.height, vp.width, vp.height - rect.height);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let visible = self.last_layout.get().visible_rows.max(1);
        // Use layout.total_rows (not view.total_rows()) so the scroll
        // ceiling accounts for @@ header lines in unified mode.
        let total = self.last_layout.get().total_rows;

        match event {
            // Click routing (#818): resolve the click through
            // `DiffView::layout` + `DiffViewGeometry::hit_test` — the
            // same geometry `render` just painted from — and report
            // what was hit in the status bar.
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let rect = Self::diff_rect(backend);
                let lh = backend.line_height();
                let geometry = self.view.layout(rect, lh);
                self.last_click = Some(match geometry.hit_test(position.x, position.y) {
                    DiffViewHit::Row { row_idx, pane } => {
                        let rows = self.view.flat_rows();
                        let kind = rows
                            .get(row_idx)
                            .map(|r| format!("{:?}", r.kind))
                            .unwrap_or_else(|| "?".into());
                        match pane {
                            Some(DiffPane::Left) => {
                                format!("clicked row {row_idx} ({kind}) [left pane]")
                            }
                            Some(DiffPane::Right) => {
                                format!("clicked row {row_idx} ({kind}) [right pane]")
                            }
                            None => format!("clicked row {row_idx} ({kind})"),
                        }
                    }
                    DiffViewHit::UnifiedHeader { hunk_idx } => {
                        format!("clicked hunk header {hunk_idx}")
                    }
                    DiffViewHit::Empty => "clicked empty area".into(),
                });
                Reaction::Redraw
            }
            // Scroll down — j or Down arrow.
            UiEvent::KeyPressed {
                key: Key::Char('j'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                ..
            } => {
                if total > visible {
                    self.view.scroll_offset = (self.view.scroll_offset + 1).min(total - visible);
                }
                Reaction::Redraw
            }

            // Scroll up — k or Up arrow.
            UiEvent::KeyPressed {
                key: Key::Char('k'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Up),
                ..
            } => {
                self.view.scroll_offset = self.view.scroll_offset.saturating_sub(1);
                Reaction::Redraw
            }

            // Page Down.
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::PageDown),
                ..
            } => {
                if total > visible {
                    self.view.scroll_offset =
                        (self.view.scroll_offset + visible).min(total - visible);
                }
                Reaction::Redraw
            }

            // Page Up.
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::PageUp),
                ..
            } => {
                self.view.scroll_offset = self.view.scroll_offset.saturating_sub(visible);
                Reaction::Redraw
            }

            // Toggle mode.
            UiEvent::KeyPressed {
                key: Key::Char('m'),
                ..
            } => {
                self.view.mode = match self.view.mode {
                    DiffMode::SideBySide => DiffMode::Unified,
                    DiffMode::Unified => DiffMode::SideBySide,
                };
                // Reset scroll when switching modes so we start at the top.
                self.view.scroll_offset = 0;
                Reaction::Redraw
            }

            // Quit.
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,

            _ => Reaction::Continue,
        }
    }
}
