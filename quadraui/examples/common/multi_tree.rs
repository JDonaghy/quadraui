//! Backend-agnostic Debug-sidebar consumer pattern: 4 `EqualShare`
//! `TreeView` sections in a `MultiSectionView`, with per-section
//! `scroll_offset` + `selected_path` owned by the host.
//!
//! Single [`AppLogic`] impl drives both the TUI runner
//! (`quadraui::tui::run`) and the GTK runner (`quadraui::gtk::run`) —
//! and any future Win-GUI / macOS runner that ships, per CLAUDE.md
//! *Cross-backend portability commitment*. The thin shells in
//! `examples/{tui,gtk}_multi_tree.rs` are each ~15 lines and call
//! `quadraui::<backend>::run(DebugSidebar::new())`.
//!
//! State + click router shape mirrors the standalone TUI / GTK
//! examples that preceded this refactor (#1, #12). The only
//! differences are:
//! - Drawing goes through `backend.draw_multi_section_view(...)` (the
//!   trait method added by #13) instead of the per-backend free
//!   function.
//! - Layout queries go through `backend.msv_layout(...)` /
//!   `backend.tree_layout(...)` (also #13) so the click router stays
//!   backend-agnostic.
//! - Mouse + keyboard events arrive as `UiEvent` (already unified
//!   across backends) instead of crossterm `MouseEventKind` or GTK
//!   `GestureDrag`.
//!
//! Controls (mirror the standalone examples):
//! - mouse click on header        activate that section
//! - mouse click on body row      activate + select
//! - mouse click on scrollbar     drag thumb / page via track
//! - `Tab` / `Shift+Tab`          cycle active section
//! - `↑` / `↓`                    scroll active section
//! - `Enter`                      select first row of active section
//! - `q` / `Esc`                  quit

use quadraui::{
    AppLogic, Backend, ButtonMask, Color, Decoration, Key, MouseButton, MsvAxis, MultiSectionView,
    MultiSectionViewHit, NamedKey, Reaction, Rect, ScrollMode, ScrollbarHit, Section, SectionBody,
    SectionHeader, SectionId, SectionSize, SelectionMode, StatusBar, StatusBarSegment, StyledText,
    TreePath, TreeRow, TreeView, TreeViewHit, UiEvent, WidgetId,
};

const STATUS_BAR_PX: f32 = 24.0;

struct TreeSection {
    id: SectionId,
    title: String,
    rows: Vec<TreeRow>,
    scroll_offset: usize,
    selected_path: Option<TreePath>,
}

struct ScrollDrag {
    section: usize,
    origin_y: f32,
    origin_offset: usize,
    travel: f32,
    max_offset: usize,
}

pub struct DebugSidebar {
    sections: Vec<TreeSection>,
    active_section: Option<usize>,
    scroll_drag: Option<ScrollDrag>,
    last_action: Option<ClickAction>,
}

#[derive(Debug, Clone, PartialEq)]
enum ClickAction {
    HeaderActivated(usize),
    BodyActivated(usize),
    RowSelected { section: usize, path: TreePath },
    ScrollbarPressed(usize),
    ScrollbarPagedUp(usize),
    ScrollbarPagedDown(usize),
}

impl DebugSidebar {
    pub fn new() -> Self {
        Self {
            sections: vec![
                tree_section("variables", "VARIABLES", &fake_rows("v", 12)),
                tree_section("watch", "WATCH", &fake_rows("w", 8)),
                tree_section("call-stack", "CALL STACK", &fake_rows("frame", 5)),
                tree_section("breakpoints", "BREAKPOINTS", &fake_rows("bp", 0)),
            ],
            active_section: None,
            scroll_drag: None,
            last_action: None,
        }
    }

    fn build_view(&self) -> MultiSectionView {
        let sections: Vec<Section> = self
            .sections
            .iter()
            .enumerate()
            .map(|(idx, s)| Section {
                id: s.id.clone(),
                header: SectionHeader {
                    title: StyledText::plain(if self.active_section == Some(idx) {
                        format!("▶ {}", s.title)
                    } else {
                        s.title.clone()
                    }),
                    show_chevron: false,
                    ..Default::default()
                },
                body: SectionBody::Tree(TreeView {
                    id: WidgetId::new(format!("{}-tree", s.id)),
                    rows: s.rows.clone(),
                    selection_mode: SelectionMode::Single,
                    selected_path: s.selected_path.clone(),
                    scroll_offset: s.scroll_offset,
                    style: Default::default(),
                    has_focus: self.active_section == Some(idx),
                }),
                aux: None,
                size: SectionSize::EqualShare,
                collapsed: false,
                min_size: None,
                max_size: None,
            })
            .collect();
        MultiSectionView {
            id: WidgetId::new("debug-sidebar"),
            sections,
            active_section: self.active_section,
            axis: MsvAxis::Vertical,
            allow_resize: false,
            allow_collapse: false,
            scroll_mode: ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        }
    }

    fn sidebar_rect(backend: &dyn Backend) -> Rect {
        let viewport = backend.viewport();
        Rect::new(
            0.0,
            0.0,
            viewport.width,
            (viewport.height - STATUS_BAR_PX).max(0.0),
        )
    }

    fn status_rect(backend: &dyn Backend) -> Rect {
        let viewport = backend.viewport();
        Rect::new(
            0.0,
            (viewport.height - STATUS_BAR_PX).max(0.0),
            viewport.width,
            STATUS_BAR_PX,
        )
    }

    fn click(&mut self, backend: &mut dyn Backend, x: f32, y: f32) -> Reaction {
        let view = self.build_view();
        let area = Self::sidebar_rect(backend);
        let layout = backend.msv_layout(area, &view);
        let action = match layout.hit_test(x, y) {
            MultiSectionViewHit::Header { section, .. } => {
                self.active_section = Some(section);
                self.sections[section].selected_path = None;
                Some(ClickAction::HeaderActivated(section))
            }
            MultiSectionViewHit::Body { section } => {
                self.active_section = Some(section);
                let body_b = layout.sections[section].body_bounds;
                let tree = match &view.sections[section].body {
                    SectionBody::Tree(t) => t.clone(),
                    _ => return Reaction::Continue,
                };
                let inner = backend.tree_layout(body_b, &tree);
                match inner.hit_test(x - body_b.x, y - body_b.y) {
                    TreeViewHit::Row(idx) => {
                        let path = tree.rows[idx].path.clone();
                        self.sections[section].selected_path = Some(path.clone());
                        Some(ClickAction::RowSelected { section, path })
                    }
                    TreeViewHit::Empty => Some(ClickAction::BodyActivated(section)),
                }
            }
            MultiSectionViewHit::Scrollbar {
                section,
                kind: ScrollbarHit::Thumb,
            } => {
                let sb = layout.sections[section]
                    .scrollbar_bounds
                    .expect("scrollbar hit implies bounds present");
                let thumb_h = layout.sections[section]
                    .thumb_bounds
                    .map(|t| t.height)
                    .unwrap_or(sb.height);
                let body_b = layout.sections[section].body_bounds;
                let viewport_rows = self.viewport_rows(backend, body_b, section, &view);
                let max_offset = self.sections[section]
                    .rows
                    .len()
                    .saturating_sub(viewport_rows);
                let travel = (sb.height - thumb_h).max(0.0);
                self.scroll_drag = Some(ScrollDrag {
                    section,
                    origin_y: y,
                    origin_offset: self.sections[section].scroll_offset,
                    travel,
                    max_offset,
                });
                Some(ClickAction::ScrollbarPressed(section))
            }
            MultiSectionViewHit::Scrollbar {
                section,
                kind: ScrollbarHit::TrackBefore,
            } => {
                let body_b = layout.sections[section].body_bounds;
                let viewport_rows = self.viewport_rows(backend, body_b, section, &view);
                self.page_scroll(section, -(viewport_rows as isize), viewport_rows);
                Some(ClickAction::ScrollbarPagedUp(section))
            }
            MultiSectionViewHit::Scrollbar {
                section,
                kind: ScrollbarHit::TrackAfter,
            } => {
                let body_b = layout.sections[section].body_bounds;
                let viewport_rows = self.viewport_rows(backend, body_b, section, &view);
                self.page_scroll(section, viewport_rows as isize, viewport_rows);
                Some(ClickAction::ScrollbarPagedDown(section))
            }
            _ => None,
        };
        if action.is_some() {
            self.last_action = action;
            Reaction::Redraw
        } else {
            Reaction::Continue
        }
    }

    /// Compute viewport rows by querying the inner tree's layout with
    /// `scroll_offset = 0`. Backend-portable: TUI and GTK both report
    /// `visible_rows.len()` against the body's height.
    fn viewport_rows(
        &self,
        backend: &dyn Backend,
        body_b: Rect,
        section: usize,
        view: &MultiSectionView,
    ) -> usize {
        let SectionBody::Tree(t) = &view.sections[section].body else {
            return 0;
        };
        let mut shadow = t.clone();
        shadow.scroll_offset = 0;
        let inner = backend.tree_layout(body_b, &shadow);
        inner.visible_rows.len()
    }

    fn drag_to(&mut self, y: f32) -> Reaction {
        let Some(drag) = &self.scroll_drag else {
            return Reaction::Continue;
        };
        if drag.travel <= 0.0 || drag.max_offset == 0 {
            return Reaction::Continue;
        }
        let dy = y - drag.origin_y;
        let drow = dy / drag.travel * drag.max_offset as f32;
        let new = (drag.origin_offset as f32 + drow).round() as i32;
        let new = new.max(0) as usize;
        let new = new.min(drag.max_offset);
        if new == self.sections[drag.section].scroll_offset {
            return Reaction::Continue;
        }
        self.sections[drag.section].scroll_offset = new;
        Reaction::Redraw
    }

    fn drag_end(&mut self) {
        self.scroll_drag = None;
    }

    fn page_scroll(&mut self, section: usize, delta: isize, viewport_rows: usize) {
        let row_count = self.sections[section].rows.len();
        let max = row_count.saturating_sub(viewport_rows) as isize;
        let cur = self.sections[section].scroll_offset as isize;
        let new = (cur + delta).max(0).min(max) as usize;
        self.sections[section].scroll_offset = new;
    }

    fn cycle_active(&mut self, delta: isize) {
        let n = self.sections.len() as isize;
        if n == 0 {
            return;
        }
        let next = match self.active_section {
            Some(i) => ((i as isize + delta).rem_euclid(n)) as usize,
            None => {
                if delta >= 0 {
                    0
                } else {
                    (n - 1) as usize
                }
            }
        };
        self.active_section = Some(next);
    }

    fn scroll_active(&mut self, backend: &mut dyn Backend, delta: isize) {
        let Some(idx) = self.active_section else {
            return;
        };
        let view = self.build_view();
        let area = Self::sidebar_rect(backend);
        let layout = backend.msv_layout(area, &view);
        let body_b = layout.sections[idx].body_bounds;
        let viewport_rows = self.viewport_rows(backend, body_b, idx, &view);
        let row_count = self.sections[idx].rows.len();
        let max = row_count.saturating_sub(viewport_rows) as isize;
        let cur = self.sections[idx].scroll_offset as isize;
        let new = (cur + delta).max(0).min(max) as usize;
        self.sections[idx].scroll_offset = new;
    }

    fn select_first_of_active(&mut self) {
        let Some(idx) = self.active_section else {
            return;
        };
        if let Some(first) = self.sections[idx].rows.first() {
            self.sections[idx].selected_path = Some(first.path.clone());
            // Reset scroll so the just-selected first row is in view.
            self.sections[idx].scroll_offset = 0;
        }
    }

    fn build_status_bar(&self) -> StatusBar {
        let active = match self.active_section {
            Some(i) => self.sections[i].id.as_str(),
            None => "<none>",
        };
        let action = match &self.last_action {
            Some(ClickAction::HeaderActivated(i)) => format!("header→{}", self.sections[*i].id),
            Some(ClickAction::BodyActivated(i)) => format!("body→{}", self.sections[*i].id),
            Some(ClickAction::RowSelected { section, path }) => {
                format!("row→{} {:?}", self.sections[*section].id, path)
            }
            Some(ClickAction::ScrollbarPressed(i)) => {
                format!("scrollbar→{}", self.sections[*i].id)
            }
            Some(ClickAction::ScrollbarPagedUp(i)) => {
                format!("page-up→{}", self.sections[*i].id)
            }
            Some(ClickAction::ScrollbarPagedDown(i)) => {
                format!("page-down→{}", self.sections[*i].id)
            }
            None => "—".to_string(),
        };
        let fg = Color::rgb(220, 220, 220);
        let bg = Color::rgb(40, 40, 60);
        StatusBar {
            id: WidgetId::new("multi-tree-status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" active: {active}  last: {action} "),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " mouse / Tab / ↑↓ / Enter / q ".into(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for DebugSidebar {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for DebugSidebar {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let sidebar = Self::sidebar_rect(backend);
        let status = Self::status_rect(backend);
        backend.draw_multi_section_view(sidebar, &self.build_view());
        let _hits = backend.draw_status_bar(status, &self.build_status_bar());
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => self.click(backend, position.x, position.y),
            UiEvent::MouseMoved {
                position,
                buttons:
                    ButtonMask {
                        left: true,
                        middle: _,
                        right: _,
                    },
            } => self.drag_to(position.y),
            UiEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => {
                self.drag_end();
                Reaction::Continue
            }
            UiEvent::KeyPressed { key, .. } => match key {
                Key::Char('q') | Key::Named(NamedKey::Escape) => Reaction::Exit,
                Key::Named(NamedKey::Tab) => {
                    self.cycle_active(1);
                    Reaction::Redraw
                }
                Key::Named(NamedKey::BackTab) => {
                    self.cycle_active(-1);
                    Reaction::Redraw
                }
                Key::Named(NamedKey::Up) => {
                    self.scroll_active(backend, -1);
                    Reaction::Redraw
                }
                Key::Named(NamedKey::Down) => {
                    self.scroll_active(backend, 1);
                    Reaction::Redraw
                }
                Key::Named(NamedKey::Enter) => {
                    self.select_first_of_active();
                    Reaction::Redraw
                }
                _ => Reaction::Continue,
            },
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}

fn tree_section(id: &str, title: &str, rows: &[TreeRow]) -> TreeSection {
    TreeSection {
        id: id.to_string(),
        title: title.to_string(),
        rows: rows.to_vec(),
        scroll_offset: 0,
        selected_path: None,
    }
}

fn fake_rows(prefix: &str, n: usize) -> Vec<TreeRow> {
    (0..n)
        .map(|i| TreeRow {
            path: vec![i as u16],
            indent: 0,
            icon: None,
            text: StyledText::plain(format!("{prefix}{i}")),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
        })
        .collect()
}
