//! Backend-agnostic app code for the Board (kanban) example
//! ([`tui_board`] / [`gtk_board`]).
//!
//! [`BoardApp`] demonstrates a three-column kanban board with cards in
//! each column, badge icons showing CI / review status, and a decision
//! hint on the selected card. It uses [`BoardModel`] for state and
//! [`Backend::draw_board`] for rendering.
//!
//! Controls:
//! - j / ↓  — move selection down
//! - k / ↑  — move selection up
//! - h / ←  — jump to previous column
//! - l / →  — jump to next column
//! - g       — jump to first card
//! - G       — jump to last card
//! - q / Esc — quit

use quadraui::{
    AppLogic, Backend, BadgeStatus, BoardAction, BoardCard, BoardColumn, BoardModel, Key, MoveDir,
    NamedKey, Reaction, Rect, Stage, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

pub struct BoardApp {
    model: BoardModel,
    last_message: String,
    /// Cached board layout from the last render pass (used for click dispatch).
    last_layout: Option<quadraui::BoardLayout>,
}

impl BoardApp {
    pub fn new() -> Self {
        Self {
            model: demo_board(),
            last_message: "j/k=move  h/l=col  g/G=top/bottom  q=quit".into(),
            last_layout: None,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let selected_title = self
            .model
            .selected_card_id
            .as_ref()
            .and_then(|id| {
                self.model.columns.iter().find_map(|col| {
                    col.cards
                        .iter()
                        .find(|c| &c.id == id)
                        .map(|c| c.title.clone())
                })
            })
            .unwrap_or_default();

        let right_text = if selected_title.is_empty() {
            " q=quit ".into()
        } else {
            format!(" {} │ q=quit ", selected_title)
        };

        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![quadraui::StatusBarSegment {
                text: format!("  {} ", self.last_message),
                fg: quadraui::Color::rgb(255, 255, 255),
                bg: quadraui::Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: right_text,
                fg: quadraui::Color::rgb(200, 200, 200),
                bg: quadraui::Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for BoardApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for BoardApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();

        // Status bar at bottom.
        let status_h = lh;
        let status_rect = Rect::new(0.0, viewport.height - status_h, viewport.width, status_h);
        let _ = backend.draw_status_bar(status_rect, &self.status_bar(), None, None);

        // Board fills the rest of the screen.
        let board_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height - status_h);
        let _layout = backend.draw_board(board_rect, &self.model);
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

            UiEvent::KeyPressed { key, .. } => {
                let action = key_to_board_action(&key);
                if let Some(act) = action {
                    self.apply_board_action(act);
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }

            UiEvent::MouseDown { position, .. } => {
                let viewport = backend.viewport();
                let lh = backend.line_height();
                let board_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height - lh);
                let layout = backend.draw_board(board_rect, &self.model);
                use quadraui::BoardHit;
                match layout.hit_test(position.x, position.y) {
                    BoardHit::Card(id) => {
                        self.model.selected_card_id = Some(id.clone());
                        let title = self
                            .model
                            .columns
                            .iter()
                            .find_map(|col| {
                                col.cards
                                    .iter()
                                    .find(|c| c.id == id)
                                    .map(|c| c.title.clone())
                            })
                            .unwrap_or_default();
                        self.last_message = format!("Selected: {}", title);
                        Reaction::Redraw
                    }
                    BoardHit::ColumnHeader(_) | BoardHit::Empty => Reaction::Continue,
                }
            }

            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}

impl BoardApp {
    fn apply_board_action(&mut self, action: BoardAction) {
        match action {
            BoardAction::MoveSelection(dir) => {
                self.model.move_selection(dir);
            }
            BoardAction::JumpToTop => {
                self.model.jump_to_top();
            }
            BoardAction::JumpToBottom => {
                self.model.jump_to_bottom();
            }
            _ => {}
        }
    }
}

fn key_to_board_action(key: &Key) -> Option<BoardAction> {
    match key {
        Key::Char('j') | Key::Named(NamedKey::Down) => {
            Some(BoardAction::MoveSelection(MoveDir::Down))
        }
        Key::Char('k') | Key::Named(NamedKey::Up) => Some(BoardAction::MoveSelection(MoveDir::Up)),
        Key::Char('h') | Key::Named(NamedKey::Left) => {
            Some(BoardAction::MoveSelection(MoveDir::Left))
        }
        Key::Char('l') | Key::Named(NamedKey::Right) => {
            Some(BoardAction::MoveSelection(MoveDir::Right))
        }
        Key::Char('g') => Some(BoardAction::JumpToTop),
        Key::Char('G') => Some(BoardAction::JumpToBottom),
        _ => None,
    }
}

// ── Demo data ───────────────────────────────────────────────────────────────

fn demo_board() -> BoardModel {
    let backlog = BoardColumn {
        id: WidgetId::new("col:backlog"),
        title: "Backlog".into(),
        cards: vec![
            BoardCard {
                id: WidgetId::new("card:1"),
                title: "Improve search indexing perf".into(),
                labels: vec!["perf".into()],
                stage_badges: vec![
                    (Stage::Plan, BadgeStatus::Passed),
                    (Stage::Work, BadgeStatus::Pending),
                ],
                assignee: Some("alice".into()),
                machine: None,
                decision_hint: None,
            },
            BoardCard {
                id: WidgetId::new("card:2"),
                title: "Add dark-mode toggle".into(),
                labels: vec!["ui".into()],
                stage_badges: vec![(Stage::Plan, BadgeStatus::Running)],
                assignee: Some("bob".into()),
                machine: None,
                decision_hint: Some("Waiting for design sign-off".into()),
            },
            BoardCard {
                id: WidgetId::new("card:3"),
                title: "Fix memory leak in parser".into(),
                labels: vec!["bug".into()],
                stage_badges: vec![],
                assignee: None,
                machine: None,
                decision_hint: None,
            },
        ],
        scroll_offset: 0,
    };

    let in_progress = BoardColumn {
        id: WidgetId::new("col:in-progress"),
        title: "In Progress".into(),
        cards: vec![
            BoardCard {
                id: WidgetId::new("card:4"),
                title: "Refactor auth middleware".into(),
                labels: vec!["refactor".into(), "security".into()],
                stage_badges: vec![
                    (Stage::Plan, BadgeStatus::Passed),
                    (Stage::Work, BadgeStatus::Running),
                    (Stage::Test, BadgeStatus::Pending),
                ],
                assignee: Some("carol".into()),
                machine: Some("ci-runner-03".into()),
                decision_hint: None,
            },
            BoardCard {
                id: WidgetId::new("card:5"),
                title: "Upgrade dependency stack".into(),
                labels: vec!["deps".into()],
                stage_badges: vec![
                    (Stage::Plan, BadgeStatus::Passed),
                    (Stage::Work, BadgeStatus::Passed),
                    (Stage::Test, BadgeStatus::Blocked),
                ],
                assignee: Some("dave".into()),
                machine: None,
                decision_hint: Some("blocked: upstream async-std 2.0 compat".into()),
            },
        ],
        scroll_offset: 0,
    };

    let review = BoardColumn {
        id: WidgetId::new("col:review"),
        title: "Review".into(),
        cards: vec![
            BoardCard {
                id: WidgetId::new("card:6"),
                title: "Paginate API list endpoints".into(),
                labels: vec!["api".into()],
                stage_badges: vec![
                    (Stage::Plan, BadgeStatus::Passed),
                    (Stage::Work, BadgeStatus::Passed),
                    (Stage::Test, BadgeStatus::Passed),
                    (Stage::Review, BadgeStatus::RequestChanges),
                ],
                assignee: Some("alice".into()),
                machine: None,
                decision_hint: Some("reviewer: needs integration test".into()),
            },
            BoardCard {
                id: WidgetId::new("card:7"),
                title: "Export CSV for analytics".into(),
                labels: vec!["feature".into()],
                stage_badges: vec![
                    (Stage::Plan, BadgeStatus::Passed),
                    (Stage::Work, BadgeStatus::Passed),
                    (Stage::Test, BadgeStatus::Passed),
                    (Stage::Review, BadgeStatus::Running),
                ],
                assignee: Some("eve".into()),
                machine: None,
                decision_hint: None,
            },
        ],
        scroll_offset: 0,
    };

    BoardModel {
        id: WidgetId::new("board:main"),
        columns: vec![backlog, in_progress, review],
        selected_card_id: Some(WidgetId::new("card:4")),
        col_scroll_offset: 0,
    }
}
