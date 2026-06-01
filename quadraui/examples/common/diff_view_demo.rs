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
use quadraui::event::{Key, NamedKey, Rect, UiEvent};
use quadraui::primitives::diff_view::{
    DiffEditability, DiffMode, DiffPane, DiffView, DiffViewLayout,
};
use quadraui::runner::{AppLogic, Reaction};
use quadraui::types::WidgetId;

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
        let rect = Rect::new(0.0, 0.0, vp.width, vp.height);
        let layout = backend.draw_diff_view(rect, &self.view);
        self.last_layout.set(layout);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let visible = self.last_layout.get().visible_rows.max(1);
        // Use layout.total_rows (not view.total_rows()) so the scroll
        // ceiling accounts for @@ header lines in unified mode.
        let total = self.last_layout.get().total_rows;

        match event {
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
