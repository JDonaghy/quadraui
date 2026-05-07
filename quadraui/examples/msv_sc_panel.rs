//! `cargo run --example msv_sc_panel --features tui`
//!
//! SC-panel consumer pattern for [`MultiSectionView`]: a
//! [`SectionAux::Input`] commit-message editor on section 0, plus N
//! collapsible [`TreeView`] sections (Changes / Staged / Worktrees…).
//! Companion to issue #2 and the consumer-state harness in
//! `quadraui/src/tui/multi_section_view.rs::tests` under "SC panel".
//!
//! The host:
//! - Owns commit-message text + caret + focus state, plus per-section
//!   `scroll_offset` / `selected_path` / `collapsed` flags. None of this
//!   state lives on the primitive — `build_view()` rebuilds a fresh
//!   `MultiSectionView` every frame.
//! - Routes clicks via [`tui_msv_layout`] + [`tui_tree_layout`] —
//!   never re-derives bounds.
//! - Routes keystrokes either to the commit input (when focused) or
//!   to scroll/select handlers (otherwise). Esc blurs the input
//!   without quitting.
//!
//! Section sizing: section 0 is `Fixed(8)` (1 header + 1 aux + 6 body
//! cells) so the commit input + Changes tree have predictable height.
//! Sections 1..N are `EqualShare` of the remainder.
//!
//! Controls:
//! - mouse click on commit input  focus input
//! - chars / Backspace / ←→        edit input (when focused)
//! - Esc                           blur input — active section restores (or quit when not focused)
//! - mouse click on chevron        toggle that section's `collapsed`
//! - mouse click on header (title) activate that section
//! - mouse click on body row       activate + select
//! - mouse click on scrollbar      drag thumb / page via track
//! - Tab / Shift+Tab               cycle active section (blurs input)
//! - ↑ / ↓                         scroll active section (when input not focused)
//! - Enter                         toggle collapse for active section (when input not focused)
//! - q                             quit (when input not focused)

use std::io;
use std::time::Duration;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
    MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::Rect;
use ratatui::Terminal;

use quadraui::tui::{draw_multi_section_view, tui_msv_layout, tui_tree_layout};
use quadraui::{
    AuxHit, Decoration, HeaderHit, InlineInput, MsvAxis, MultiSectionView, MultiSectionViewHit,
    ScrollMode, ScrollbarHit, Section, SectionAux, SectionBody, SectionHeader, SectionId,
    SectionSize, SelectionMode, StyledText, Theme, TreePath, TreeRow, TreeView, TreeViewHit,
    WidgetId,
};

/// Per-section host state — same shape as #1's `TreeSection` plus a
/// `collapsed` flag the host owns.
struct SCSection {
    id: SectionId,
    title: String,
    rows: Vec<TreeRow>,
    scroll_offset: usize,
    selected_path: Option<TreePath>,
    collapsed: bool,
}

struct ScrollDrag {
    section: usize,
    origin_y: u16,
    origin_offset: usize,
    viewport_rows: usize,
}

/// SC-panel consumer state. Commit input on top of section 0, plus N
/// collapsible Tree sections. Section-0 body remains a Tree (Changes);
/// the input is its `aux`, not its body.
pub struct SCSidebar {
    commit_message: String,
    commit_caret: usize,
    commit_input_active: bool,
    sections: Vec<SCSection>,
    active_section: Option<usize>,
    previous_active: Option<usize>,
    scroll_drag: Option<ScrollDrag>,
}

impl SCSidebar {
    pub fn new() -> Self {
        Self {
            commit_message: String::new(),
            commit_caret: 0,
            commit_input_active: false,
            sections: vec![
                sc_section("changes", "CHANGES", &fake_rows("M ", "src/", 8)),
                sc_section("staged", "STAGED", &fake_rows("A ", "tests/", 4)),
                sc_section("worktrees", "WORKTREES", &fake_rows("wt-", "", 2)),
            ],
            active_section: None,
            previous_active: None,
            scroll_drag: None,
        }
    }

    /// Build a fresh [`MultiSectionView`] from current host state.
    /// Section 0 = Fixed(8) with aux=Input + body=Tree(Changes);
    /// sections 1..N = EqualShare with body=Tree. All sections show
    /// chevrons and are collapsible.
    fn build_view(&self) -> MultiSectionView {
        let sections: Vec<Section> = self
            .sections
            .iter()
            .enumerate()
            .map(|(idx, s)| {
                let aux = if idx == 0 {
                    Some(SectionAux::Input(InlineInput {
                        id: WidgetId::new("commit-input"),
                        text: self.commit_message.clone(),
                        caret: self.commit_caret,
                        placeholder: Some("Commit message".to_string()),
                        has_focus: self.commit_input_active,
                    }))
                } else {
                    None
                };
                let size = if idx == 0 {
                    SectionSize::Fixed(8)
                } else {
                    SectionSize::EqualShare
                };
                Section {
                    id: s.id.clone(),
                    header: SectionHeader {
                        title: StyledText::plain(s.title.clone()),
                        show_chevron: true,
                        ..Default::default()
                    },
                    body: SectionBody::Tree(TreeView {
                        id: WidgetId::new(format!("{}-tree", s.id)),
                        rows: s.rows.clone(),
                        selection_mode: SelectionMode::Single,
                        selected_path: s.selected_path.clone(),
                        scroll_offset: s.scroll_offset,
                        style: Default::default(),
                        has_focus: !self.commit_input_active && self.active_section == Some(idx),
                    }),
                    aux,
                    size,
                    collapsed: s.collapsed,
                    min_size: None,
                    max_size: None,
                }
            })
            .collect();
        MultiSectionView {
            id: WidgetId::new("sc-sidebar"),
            sections,
            active_section: self.active_section,
            axis: MsvAxis::Vertical,
            allow_resize: false,
            allow_collapse: true,
            scroll_mode: ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        }
    }

    /// Route a primary mouse-down at (x, y).
    pub fn click(&mut self, x: u16, y: u16, area: Rect) -> ClickAction {
        let view = self.build_view();
        let layout = tui_msv_layout(&view, area);
        match layout.hit_test(x as f32, y as f32) {
            MultiSectionViewHit::Aux {
                section: _,
                kind: AuxHit::Input,
            } => {
                self.previous_active = self.active_section;
                self.commit_input_active = true;
                self.active_section = None;
                ClickAction::InputFocused
            }
            MultiSectionViewHit::Header {
                section,
                kind: HeaderHit::Chevron,
            } => {
                self.commit_input_active = false;
                self.active_section = Some(section);
                self.sections[section].collapsed = !self.sections[section].collapsed;
                ClickAction::HeaderToggled(section)
            }
            MultiSectionViewHit::Header {
                section,
                kind: HeaderHit::TitleArea,
            } => {
                self.commit_input_active = false;
                self.active_section = Some(section);
                self.sections[section].selected_path = None;
                ClickAction::HeaderActivated(section)
            }
            MultiSectionViewHit::Header { .. } => ClickAction::None,
            MultiSectionViewHit::Body { section } => {
                self.commit_input_active = false;
                self.active_section = Some(section);
                let body_b = layout.sections[section].body_bounds;
                let tree = match &view.sections[section].body {
                    SectionBody::Tree(t) => t.clone(),
                    _ => return ClickAction::None,
                };
                let body_area = Rect::new(
                    body_b.x.round() as u16,
                    body_b.y.round() as u16,
                    body_b.width.round() as u16,
                    body_b.height.round() as u16,
                );
                let inner = tui_tree_layout(&tree, body_area);
                match inner.hit_test(x as f32 - body_b.x, y as f32 - body_b.y) {
                    TreeViewHit::Row(idx) => {
                        let path = tree.rows[idx].path.clone();
                        self.sections[section].selected_path = Some(path.clone());
                        ClickAction::RowSelected { section, path }
                    }
                    TreeViewHit::Empty => ClickAction::BodyActivated(section),
                }
            }
            MultiSectionViewHit::Scrollbar {
                section,
                kind: ScrollbarHit::Thumb,
            } => {
                let viewport_rows = layout.sections[section].body_bounds.height as usize;
                self.scroll_drag = Some(ScrollDrag {
                    section,
                    origin_y: y,
                    origin_offset: self.sections[section].scroll_offset,
                    viewport_rows,
                });
                ClickAction::ScrollbarPressed(section)
            }
            MultiSectionViewHit::Scrollbar {
                section,
                kind: ScrollbarHit::TrackBefore,
            } => {
                let body_h = layout.sections[section].body_bounds.height as usize;
                self.page_scroll(section, -(body_h as isize), body_h);
                ClickAction::ScrollbarPagedUp(section)
            }
            MultiSectionViewHit::Scrollbar {
                section,
                kind: ScrollbarHit::TrackAfter,
            } => {
                let body_h = layout.sections[section].body_bounds.height as usize;
                self.page_scroll(section, body_h as isize, body_h);
                ClickAction::ScrollbarPagedDown(section)
            }
            _ => {
                // Click outside any interactive region — defocus input.
                self.commit_input_active = false;
                ClickAction::None
            }
        }
    }

    pub fn drag_to(&mut self, y: u16) -> bool {
        let Some(drag) = &self.scroll_drag else {
            return false;
        };
        let dy = y as i32 - drag.origin_y as i32;
        let row_count = self.sections[drag.section].rows.len();
        let max_offset = row_count.saturating_sub(drag.viewport_rows);
        let new = (drag.origin_offset as i32 + dy).max(0) as usize;
        let new = new.min(max_offset);
        let changed = new != self.sections[drag.section].scroll_offset;
        self.sections[drag.section].scroll_offset = new;
        changed
    }

    pub fn drag_end(&mut self) {
        self.scroll_drag = None;
    }

    fn page_scroll(&mut self, section: usize, delta: isize, viewport_rows: usize) {
        let row_count = self.sections[section].rows.len();
        let max = row_count.saturating_sub(viewport_rows) as isize;
        let cur = self.sections[section].scroll_offset as isize;
        let new = (cur + delta).max(0).min(max) as usize;
        self.sections[section].scroll_offset = new;
    }

    pub fn cycle_active(&mut self, delta: isize) {
        self.commit_input_active = false;
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

    pub fn scroll_active(&mut self, area: Rect, delta: isize) -> bool {
        let Some(idx) = self.active_section else {
            return false;
        };
        let viewport_rows = {
            let view = self.build_view();
            let layout = tui_msv_layout(&view, area);
            layout.sections[idx].body_bounds.height as usize
        };
        let row_count = self.sections[idx].rows.len();
        let max = row_count.saturating_sub(viewport_rows) as isize;
        let cur = self.sections[idx].scroll_offset as isize;
        let new = (cur + delta).max(0).min(max) as usize;
        let changed = new != self.sections[idx].scroll_offset;
        self.sections[idx].scroll_offset = new;
        changed
    }

    /// Toggle collapsed for the active section (Enter when input is
    /// not focused).
    pub fn toggle_active_collapsed(&mut self) {
        if let Some(idx) = self.active_section {
            self.sections[idx].collapsed = !self.sections[idx].collapsed;
        }
    }

    // ── Input keystroke routing ────────────────────────────────────────

    pub fn input_active(&self) -> bool {
        self.commit_input_active
    }

    pub fn type_char(&mut self, c: char) -> bool {
        if !self.commit_input_active {
            return false;
        }
        let mut chars: Vec<char> = self.commit_message.chars().collect();
        chars.insert(self.commit_caret.min(chars.len()), c);
        self.commit_message = chars.into_iter().collect();
        self.commit_caret += 1;
        true
    }

    pub fn backspace(&mut self) -> bool {
        if !self.commit_input_active || self.commit_caret == 0 {
            return false;
        }
        let mut chars: Vec<char> = self.commit_message.chars().collect();
        if self.commit_caret > chars.len() {
            self.commit_caret = chars.len();
        }
        chars.remove(self.commit_caret - 1);
        self.commit_message = chars.into_iter().collect();
        self.commit_caret -= 1;
        true
    }

    pub fn move_caret(&mut self, dx: isize) -> bool {
        if !self.commit_input_active {
            return false;
        }
        let len = self.commit_message.chars().count();
        let new = (self.commit_caret as isize + dx).max(0).min(len as isize) as usize;
        let changed = new != self.commit_caret;
        self.commit_caret = new;
        changed
    }

    pub fn blur_input(&mut self) {
        self.commit_input_active = false;
        self.active_section = self.previous_active.or(Some(0));
    }
}

impl Default for SCSidebar {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClickAction {
    InputFocused,
    HeaderToggled(usize),
    HeaderActivated(usize),
    BodyActivated(usize),
    RowSelected { section: usize, path: TreePath },
    ScrollbarPressed(usize),
    ScrollbarPagedUp(usize),
    ScrollbarPagedDown(usize),
    None,
}

fn sc_section(id: &str, title: &str, rows: &[TreeRow]) -> SCSection {
    SCSection {
        id: id.to_string(),
        title: title.to_string(),
        rows: rows.to_vec(),
        scroll_offset: 0,
        selected_path: None,
        collapsed: false,
    }
}

fn fake_rows(prefix: &str, dir: &str, n: usize) -> Vec<TreeRow> {
    (0..n)
        .map(|i| TreeRow {
            path: vec![i as u16],
            indent: 0,
            icon: None,
            text: StyledText::plain(format!("{prefix}{dir}file_{i}.rs")),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: None,
        })
        .collect()
}

// ── Runner ─────────────────────────────────────────────────────────────────

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let theme = Theme::default();
    let mut sidebar = SCSidebar::new();
    let mut last_action: Option<ClickAction> = None;

    let result = run_loop(&mut terminal, &theme, &mut sidebar, &mut last_action);

    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = terminal.show_cursor();

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    theme: &Theme,
    sidebar: &mut SCSidebar,
    last_action: &mut Option<ClickAction>,
) -> io::Result<()> {
    loop {
        let mut sidebar_area = Rect::default();
        terminal.draw(|frame| {
            let size = frame.area();
            sidebar_area = Rect::new(0, 0, size.width, size.height.saturating_sub(1));

            let view = sidebar.build_view();
            draw_multi_section_view(
                frame.buffer_mut(),
                sidebar_area,
                &view,
                theme,
                /* nerd_fonts */ false,
            );

            let status = format_status(sidebar, last_action.as_ref());
            let status_y = size.height.saturating_sub(1);
            let buf = frame.buffer_mut();
            for (i, ch) in status.chars().enumerate() {
                let x = i as u16;
                if x >= size.width {
                    break;
                }
                buf[(x, status_y)].set_char(ch);
            }
        })?;

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if sidebar.input_active() {
                    // Input mode: chars / Backspace / arrows / Esc-to-blur.
                    match key.code {
                        KeyCode::Esc => sidebar.blur_input(),
                        KeyCode::Backspace => {
                            sidebar.backspace();
                        }
                        KeyCode::Left => {
                            sidebar.move_caret(-1);
                        }
                        KeyCode::Right => {
                            sidebar.move_caret(1);
                        }
                        KeyCode::Char(c) => {
                            sidebar.type_char(c);
                        }
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Tab => sidebar.cycle_active(1),
                        KeyCode::BackTab => sidebar.cycle_active(-1),
                        KeyCode::Up => {
                            sidebar.scroll_active(sidebar_area, -1);
                        }
                        KeyCode::Down => {
                            sidebar.scroll_active(sidebar_area, 1);
                        }
                        KeyCode::Enter => sidebar.toggle_active_collapsed(),
                        _ => {}
                    }
                }
            }
            Event::Mouse(m) => match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    *last_action = Some(sidebar.click(m.column, m.row, sidebar_area));
                }
                MouseEventKind::Drag(MouseButton::Left) => {
                    sidebar.drag_to(m.row);
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    sidebar.drag_end();
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn format_status(sidebar: &SCSidebar, last: Option<&ClickAction>) -> String {
    let active = if sidebar.input_active() {
        "input".to_string()
    } else {
        match sidebar.active_section {
            Some(i) => sidebar.sections[i].id.clone(),
            None => "<none>".to_string(),
        }
    };
    let action = match last {
        Some(ClickAction::InputFocused) => "input focused".to_string(),
        Some(ClickAction::HeaderToggled(i)) => format!("toggle→{}", sidebar.sections[*i].id),
        Some(ClickAction::HeaderActivated(i)) => {
            format!("header→{}", sidebar.sections[*i].id)
        }
        Some(ClickAction::BodyActivated(i)) => format!("body→{}", sidebar.sections[*i].id),
        Some(ClickAction::RowSelected { section, path }) => {
            format!("row→{} {:?}", sidebar.sections[*section].id, path)
        }
        Some(ClickAction::ScrollbarPressed(i)) => {
            format!("scrollbar→{}", sidebar.sections[*i].id)
        }
        Some(ClickAction::ScrollbarPagedUp(i)) => {
            format!("page-up→{}", sidebar.sections[*i].id)
        }
        Some(ClickAction::ScrollbarPagedDown(i)) => {
            format!("page-down→{}", sidebar.sections[*i].id)
        }
        Some(ClickAction::None) => "inert".to_string(),
        None => "—".to_string(),
    };
    format!(
        " active: {active}  last: {action}  (click input/chevron, Tab/↑↓/Enter, Esc blur, q quit) "
    )
}
