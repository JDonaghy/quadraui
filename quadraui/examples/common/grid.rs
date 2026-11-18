//! Backend-agnostic app code for the grid layout example
//! ([`tui_grid`] / [`gtk_grid`]).
//!
//! [`GridApp`] demonstrates [`quadraui::layout::resolve_grid_tracks`] (and
//! its [`quadraui::layout::GridTrack`] vocabulary — `Fixed`/`Percent`/
//! `Weight`/`Content`) by tiling six metric cards into a 2-row-by-3-column
//! dashboard: every row and column here is an equal-weight
//! `GridTrack::Weight(1.0)` track, so the six [`Button`] tiles always
//! split the available body evenly, regardless of viewport size — the
//! same track-resolution math a caller wanting a `Fixed` sidebar column
//! or a `Percent` header row would reach for instead.
//!
//! Controls:
//! - arrow keys    move focus between tiles (2-D, row/column aware)
//! - Enter          activate the focused tile
//! - click a tile   focus + activate it
//! - q / Esc        quit

use quadraui::layout::{resolve_grid_tracks, GridTrack};
use quadraui::{
    AppLogic, Backend, Button, ButtonHit, Color, InteractionState, Key, NamedKey, Reaction, Rect,
    StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

const ROWS: usize = 2;
const COLS: usize = 3;

/// Dashboard tile labels, row-major (`TILES[row * COLS + col]`) — matches
/// [`resolve_grid_tracks`]'s own `[row][col]` ordering once flattened.
const TILES: [&str; ROWS * COLS] = [
    "CPU 42%",
    "Memory 61%",
    "Disk 78%",
    "Network 12%",
    "Requests 230/s",
    "Errors 3",
];

pub struct GridApp {
    focused: usize,
    last_message: String,
}

impl GridApp {
    pub fn new() -> Self {
        Self {
            focused: 0,
            last_message: "arrows=move Enter=activate q=quit".into(),
        }
    }

    /// Body rect above the status bar.
    fn body_rect(backend: &dyn Backend) -> Rect {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        Rect::new(0.0, 0.0, viewport.width, viewport.height - lh)
    }

    /// Six cell rects, 2 rows x 3 cols, each an equal-weight
    /// [`GridTrack::Weight`] track — flattened row-major to match
    /// [`TILES`]'s own order.
    fn cells(body: Rect) -> Vec<Rect> {
        let columns = vec![GridTrack::Weight(1.0); COLS];
        let rows = vec![GridTrack::Weight(1.0); ROWS];
        resolve_grid_tracks(&columns, &rows, body)
            .into_iter()
            .flatten()
            .collect()
    }

    /// Shrinks a cell by a small margin so adjacent tiles never visually
    /// touch. [`resolve_grid_tracks`] tiles cells edge-to-edge by design
    /// (see its own doc) — the gutter between tiles is this app's job,
    /// not the layout helper's.
    fn inset(rect: Rect, lh: f32) -> Rect {
        let margin = (lh * 0.5).max(1.0);
        Rect::new(
            rect.x + margin,
            rect.y + margin,
            (rect.width - margin * 2.0).max(0.0),
            (rect.height - margin * 2.0).max(0.0),
        )
    }

    fn tile(&self, index: usize) -> Button {
        let mut button = Button::new(WidgetId::new(format!("tile-{index}")), TILES[index]);
        button.focused = index == self.focused;
        button
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!("  {} ", self.last_message),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " q=quit ".into(),
                fg: Color::rgb(200, 200, 200),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn activate_focused(&mut self) {
        self.last_message = format!("Activated {}", TILES[self.focused]);
    }
}

impl Default for GridApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for GridApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();

        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );

        let body = Self::body_rect(backend);
        for (index, cell) in Self::cells(body).into_iter().enumerate() {
            let rect = Self::inset(cell, lh);
            let _ = backend.draw_button(rect, &self.tile(index));
        }
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,

            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Left),
                ..
            } => {
                if !self.focused.is_multiple_of(COLS) {
                    self.focused -= 1;
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Right),
                ..
            } => {
                if self.focused % COLS < COLS - 1 {
                    self.focused += 1;
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Up),
                ..
            } => {
                if self.focused >= COLS {
                    self.focused -= COLS;
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                ..
            } => {
                if self.focused + COLS < ROWS * COLS {
                    self.focused += COLS;
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter),
                ..
            } => {
                self.activate_focused();
                Reaction::Redraw
            }

            UiEvent::MouseDown { position, .. } => {
                let body = Self::body_rect(backend);
                let lh = backend.line_height();
                for (index, cell) in Self::cells(body).into_iter().enumerate() {
                    let rect = Self::inset(cell, lh);
                    let layout = self.tile(index).layout(rect);
                    if layout.hit_test(position) == ButtonHit::Body {
                        self.focused = index;
                        self.activate_focused();
                        return Reaction::Redraw;
                    }
                }
                Reaction::Continue
            }

            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
