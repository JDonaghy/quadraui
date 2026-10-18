//! `DiffView` demo — adapted from
//! `quadraui/examples/common/diff_view_demo.rs`.
//!
//! Two variants of the same diff: [`DiffMode::SideBySide`] and
//! [`DiffMode::Unified`] — `j`/`k` scroll, clicking a row reports which
//! pane (and hunk, in Unified) it landed in.

use quadraui::{
    Backend, BackendCaps, Color, DiffEditability, DiffMode, DiffPane, DiffView, DiffViewHit,
    InteractionState, Key, MouseButton, Reaction, Rect, StatusBar, StatusBarSegment, UiEvent,
    WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("diff_view.rs");

// gallery:begin
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

pub struct DiffViewDemo {
    views: Vec<DiffView>,
    last_click: Vec<Option<String>>,
    last_visible_rows: Vec<usize>,
    last_total_rows: Vec<usize>,
}

impl DiffViewDemo {
    pub fn new() -> Self {
        let hunks = quadraui::compute_hunks(LEFT, RIGHT);
        let side_by_side = DiffView {
            id: WidgetId::new("gallery:diff-view:side-by-side"),
            left: LEFT.to_string(),
            right: RIGHT.to_string(),
            left_label: Some("original".to_string()),
            right_label: Some("modified".to_string()),
            hunks: hunks.clone(),
            mode: DiffMode::SideBySide,
            editability: DiffEditability::ReadOnly,
            scroll_offset: 0,
            focused_pane: DiffPane::Left,
            has_focus: true,
        };
        let mut unified = side_by_side.clone();
        unified.id = WidgetId::new("gallery:diff-view:unified");
        unified.mode = DiffMode::Unified;
        Self {
            views: vec![side_by_side, unified],
            last_click: vec![None, None],
            last_visible_rows: vec![24, 24],
            last_total_rows: vec![0, 0],
        }
    }

    fn diff_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0))
    }

    fn status_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y + area.height - lh, area.width, lh)
    }

    fn status(&self, variant: usize) -> StatusBar {
        let msg = match &self.last_click[variant] {
            Some(m) => format!(" {m} "),
            None => " click a row — j/k scroll ".into(),
        };
        StatusBar {
            id: WidgetId::new("gallery:diff-view:status"),
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

impl Default for DiffViewDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for DiffViewDemo {
    fn name(&self) -> &'static str {
        "Diff View"
    }

    fn group(&self) -> &'static str {
        "Data"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Side by side", "Unified"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let rect = Self::diff_rect(area, backend);
        let _layout = backend.draw_diff_view(rect, &self.views[variant]);

        let status_rect = Self::status_rect(area, backend);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status(variant),
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
        let rect = Self::diff_rect(area, backend);
        // Refresh the cached layout extent (visible/total rows) before
        // acting on a scroll key — mirrors `DiffViewApp::last_layout`,
        // but recomputed per-call since `Demo::handle` takes `&mut self`
        // without a cached-layout slot of its own.
        let lh = backend.line_height();
        let geometry = self.views[variant].layout(rect, lh);
        self.last_visible_rows[variant] = geometry.visible_rows.max(1);
        self.last_total_rows[variant] = geometry.total_rows;
        let visible = self.last_visible_rows[variant];
        let total = self.last_total_rows[variant];

        match event {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                self.last_click[variant] = Some(match geometry.hit_test(position.x, position.y) {
                    DiffViewHit::Row { row_idx, pane } => {
                        let rows = self.views[variant].flat_rows();
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
            UiEvent::KeyPressed {
                key: Key::Char('j'),
                ..
            } => {
                if total > visible {
                    self.views[variant].scroll_offset =
                        (self.views[variant].scroll_offset + 1).min(total - visible);
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('k'),
                ..
            } => {
                self.views[variant].scroll_offset =
                    self.views[variant].scroll_offset.saturating_sub(1);
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        serde_json::to_value(&self.views[variant]).unwrap_or(serde_json::Value::Null)
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
    fn variants_have_matching_modes() {
        let demo = DiffViewDemo::new();
        assert_eq!(demo.views[0].mode, DiffMode::SideBySide);
        assert_eq!(demo.views[1].mode, DiffMode::Unified);
    }

    #[test]
    fn both_variants_share_the_same_hunks() {
        let demo = DiffViewDemo::new();
        assert_eq!(demo.views[0].hunks, demo.views[1].hunks);
    }
}

#[cfg(all(test, feature = "tui"))]
mod tui_tests {
    use super::*;

    #[test]
    fn j_key_scrolls_down_one_row() {
        let mut demo = DiffViewDemo::new();
        let mut backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 4.0);
        let event = UiEvent::KeyPressed {
            key: Key::Char('j'),
            modifiers: Default::default(),
            repeat: false,
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.views[0].scroll_offset, 1);
    }
}
