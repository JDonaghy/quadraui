//! `Board` (kanban) demo — adapted from
//! `quadraui/examples/common/board_app.rs`.
//!
//! Two variants: a populated sprint board (three columns, badges, a
//! blocked-card hint) and an empty board (every column has zero
//! cards) — the same [`BoardModel`] shape, showing it handles the
//! nothing-to-select case gracefully.

use quadraui::{
    Backend, BackendCaps, BadgeStatus, BoardAction, BoardCard, BoardColumn, BoardHit, BoardModel,
    CardBadge, Color, InteractionState, Key, MoveDir, Reaction, Rect, StatusBar, StatusBarSegment,
    UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("board.rs");

// gallery:begin
pub struct BoardDemo {
    models: Vec<BoardModel>,
    last_message: String,
}

impl BoardDemo {
    pub fn new() -> Self {
        Self {
            models: vec![sprint_board(), empty_board()],
            last_message: "j/k=move  h/l=col  g/G=top/bottom".into(),
        }
    }

    fn model(&self, variant: usize) -> &BoardModel {
        &self.models[variant.min(self.models.len() - 1)]
    }

    fn model_mut(&mut self, variant: usize) -> &mut BoardModel {
        let idx = variant.min(self.models.len() - 1);
        &mut self.models[idx]
    }

    fn board_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0))
    }

    fn status_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y + area.height - lh, area.width, lh)
    }

    fn status(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("gallery:board:status"),
            left_segments: vec![StatusBarSegment {
                text: format!("  {} ", self.last_message),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }
}

impl Default for BoardDemo {
    fn default() -> Self {
        Self::new()
    }
}

fn key_to_board_action(key: &Key) -> Option<BoardAction> {
    match key {
        Key::Char('j') => Some(BoardAction::MoveSelection(MoveDir::Down)),
        Key::Char('k') => Some(BoardAction::MoveSelection(MoveDir::Up)),
        Key::Char('h') => Some(BoardAction::MoveSelection(MoveDir::Left)),
        Key::Char('l') => Some(BoardAction::MoveSelection(MoveDir::Right)),
        Key::Char('g') => Some(BoardAction::JumpToTop),
        Key::Char('G') => Some(BoardAction::JumpToBottom),
        _ => None,
    }
}

fn badge(label: &str, status: BadgeStatus) -> CardBadge {
    CardBadge {
        label: label.into(),
        status,
    }
}

fn sprint_board() -> BoardModel {
    let backlog = BoardColumn {
        id: WidgetId::new("gallery:board:col:backlog"),
        title: "Backlog".into(),
        cards: vec![
            BoardCard {
                id: WidgetId::new("gallery:board:card:1"),
                title: "Improve search indexing perf".into(),
                labels: vec!["perf".into()],
                badges: vec![
                    badge("P", BadgeStatus::Passed),
                    badge("W", BadgeStatus::Pending),
                ],
                hint: None,
            },
            BoardCard {
                id: WidgetId::new("gallery:board:card:2"),
                title: "Fix memory leak in parser".into(),
                labels: vec!["bug".into()],
                badges: vec![],
                hint: None,
            },
        ],
        scroll_offset: 0,
    };
    let in_progress = BoardColumn {
        id: WidgetId::new("gallery:board:col:in-progress"),
        title: "In Progress".into(),
        cards: vec![BoardCard {
            id: WidgetId::new("gallery:board:card:3"),
            title: "Upgrade dependency stack".into(),
            labels: vec!["deps".into()],
            badges: vec![
                badge("P", BadgeStatus::Passed),
                badge("W", BadgeStatus::Passed),
                badge("T", BadgeStatus::Blocked),
            ],
            hint: Some("blocked: upstream async-std 2.0 compat".into()),
        }],
        scroll_offset: 0,
    };
    let review = BoardColumn {
        id: WidgetId::new("gallery:board:col:review"),
        title: "Review".into(),
        cards: vec![BoardCard {
            id: WidgetId::new("gallery:board:card:4"),
            title: "Paginate API list endpoints".into(),
            labels: vec!["api".into()],
            badges: vec![
                badge("P", BadgeStatus::Passed),
                badge("W", BadgeStatus::Passed),
                badge("R", BadgeStatus::Warning),
            ],
            hint: Some("reviewer: needs integration test".into()),
        }],
        scroll_offset: 0,
    };
    BoardModel {
        id: WidgetId::new("gallery:board:main"),
        columns: vec![backlog, in_progress, review],
        selected_card_id: Some(WidgetId::new("gallery:board:card:3")),
        col_scroll_offset: 0,
    }
}

fn empty_board() -> BoardModel {
    BoardModel {
        id: WidgetId::new("gallery:board:empty"),
        columns: vec![
            BoardColumn {
                id: WidgetId::new("gallery:board:empty:todo"),
                title: "To Do".into(),
                cards: vec![],
                scroll_offset: 0,
            },
            BoardColumn {
                id: WidgetId::new("gallery:board:empty:done"),
                title: "Done".into(),
                cards: vec![],
                scroll_offset: 0,
            },
        ],
        selected_card_id: None,
        col_scroll_offset: 0,
    }
}

impl Demo for BoardDemo {
    fn name(&self) -> &'static str {
        "Board"
    }

    fn group(&self) -> &'static str {
        "Data"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Sprint board", "Empty board"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let board_rect = Self::board_rect(area, backend);
        let _ = backend.draw_board(board_rect, self.model(variant));

        let status_rect = Self::status_rect(area, backend);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status(),
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
            UiEvent::KeyPressed { key, .. } => {
                let Some(action) = key_to_board_action(key) else {
                    return Reaction::Continue;
                };
                let model = self.model_mut(variant);
                match action {
                    BoardAction::MoveSelection(dir) => model.move_selection(dir),
                    BoardAction::JumpToTop => model.jump_to_top(),
                    BoardAction::JumpToBottom => model.jump_to_bottom(),
                    _ => {}
                }
                Reaction::Redraw
            }
            UiEvent::MouseDown { position, .. } => {
                let board_rect = Self::board_rect(area, backend);
                let layout = backend.board_layout(board_rect, self.model(variant));
                match layout.hit_test(position.x, position.y) {
                    BoardHit::Card(id) => {
                        let title = self
                            .model(variant)
                            .columns
                            .iter()
                            .find_map(|col| {
                                col.cards
                                    .iter()
                                    .find(|c| c.id == id)
                                    .map(|c| c.title.clone())
                            })
                            .unwrap_or_default();
                        self.model_mut(variant).selected_card_id = Some(id);
                        self.last_message = format!("Selected: {title}");
                        Reaction::Redraw
                    }
                    BoardHit::ColumnHeader(_) | BoardHit::Empty => Reaction::Continue,
                }
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        serde_json::to_value(self.model(variant)).unwrap_or(serde_json::Value::Null)
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
    fn empty_board_variant_has_no_cards_and_no_selection() {
        let demo = BoardDemo::new();
        let model = demo.model(1);
        assert!(model.columns.iter().all(|c| c.cards.is_empty()));
        assert_eq!(model.selected_card_id, None);
    }

    #[test]
    fn h_key_moves_selection_into_the_previous_column() {
        let mut demo = BoardDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        let area = Rect::new(0.0, 0.0, 80.0, 20.0);
        // The demo starts with card:3 selected, in the "In Progress"
        // column — `h` moves left into "Backlog", landing on its first
        // card (card:1) since the in-progress column only has one row.
        let event = UiEvent::KeyPressed {
            key: Key::Char('h'),
            modifiers: Default::default(),
            repeat: false,
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(
            demo.model(0).selected_card_id,
            Some(WidgetId::new("gallery:board:card:1"))
        );
    }
}
