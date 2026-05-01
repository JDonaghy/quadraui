//! `cargo run --example msv_multi_tree --features tui`
//!
//! Debug-sidebar consumer pattern: 4 `EqualShare` `TreeView` sections
//! stacked in a [`MultiSectionView`], each with its own `scroll_offset`
//! and `selected_path` owned by the host.
//!
//! This is the canonical recipe vimcode's Debug sidebar
//! (Variables / Watch / Call Stack / Breakpoints) wants. Companion to
//! issue #1 in this repo and the harness extensions in
//! `quadraui/src/tui/multi_section_view.rs::tests` under
//! "Consumer-state round-trip harness".
//!
//! The host:
//! - Owns per-section scroll + selection state in [`DebugSidebar`].
//! - Builds a fresh [`MultiSectionView`] from that state every frame
//!   ([`DebugSidebar::build_view`]) — primitives are declarative, not
//!   retained.
//! - Routes clicks via [`tui_msv_layout`] + [`tui_tree_layout`] —
//!   never re-derives bounds.
//! - Updates only the targeted section's `scroll_offset` on scrollbar
//!   drag (the per-consumer state lives on `DebugSidebar`, NOT
//!   smuggled back into the primitive via `Cell<T>` engine fields).
//!
//! Controls:
//! - `Tab` / `Shift+Tab`     cycle active section
//! - `↑` / `↓`               scroll active section
//! - `Enter`                 toggle selection on first row of active section
//! - mouse click on header   activate that section
//! - mouse click on body row activate + select
//! - mouse click on scrollbar track above thumb → page up that section
//! - mouse click on scrollbar track below thumb → page down that section
//! - mouse drag on scrollbar thumb → update only that section's `scroll_offset`
//! - `q` / `Esc`             quit

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
    Decoration, MsvAxis, MultiSectionView, MultiSectionViewHit, ScrollMode, ScrollbarHit, Section,
    SectionBody, SectionHeader, SectionId, SectionSize, SelectionMode, StyledText, Theme, TreePath,
    TreeRow, TreeView, TreeViewHit, WidgetId,
};

/// Per-section consumer state. The host owns scroll + selection;
/// the [`MultiSectionView`] is rebuilt every frame from this struct.
struct TreeSection {
    id: SectionId,
    title: String,
    rows: Vec<TreeRow>,
    scroll_offset: usize,
    selected_path: Option<TreePath>,
}

/// Active drag captured on `MouseDown` over a scrollbar thumb. We
/// snapshot the section index, the y the drag began at, the
/// `scroll_offset` at that moment, and the section's viewport row
/// count. On every subsequent `MouseMoved` we recompute the new
/// offset as `origin_offset + (y - origin_y)` clamped to
/// `[0, rows.len() - viewport_rows]` — NOT `[0, rows.len() - 1]`,
/// which would let the user drag past the last viewport-full of
/// rows into a state where only the trailing row is visible.
/// Releasing the button (or moving outside) clears the capture.
struct ScrollDrag {
    section: usize,
    origin_y: u16,
    origin_offset: usize,
    viewport_rows: usize,
}

/// The N-tree-section debug-sidebar consumer. State lives here, NOT
/// inside the primitive. Paint and click both consume the layout
/// produced by [`tui_msv_layout`] — that's the source-of-truth contract
/// `MultiSectionView` exists to enforce.
pub struct DebugSidebar {
    sections: Vec<TreeSection>,
    active_section: Option<usize>,
    scroll_drag: Option<ScrollDrag>,
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
        }
    }

    /// Build the declarative [`MultiSectionView`] for this frame.
    /// Every section is `EqualShare`, no aux row, headers without
    /// chevrons (Debug-sidebar style).
    fn build_view(&self) -> MultiSectionView {
        let sections: Vec<Section> = self
            .sections
            .iter()
            .enumerate()
            .map(|(idx, s)| Section {
                id: s.id.clone(),
                header: SectionHeader {
                    title: StyledText::plain(s.title.clone()),
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

    /// Route a primary mouse-down at (x, y) inside `area`. Header click
    /// activates the section without selecting; body-row click activates
    /// AND selects; body click on an empty section activates only;
    /// scrollbar click captures a drag.
    pub fn click(&mut self, x: u16, y: u16, area: Rect) -> ClickAction {
        let view = self.build_view();
        let layout = tui_msv_layout(&view, area);
        match layout.hit_test(x as f32, y as f32) {
            MultiSectionViewHit::Header { section, .. } => {
                self.active_section = Some(section);
                self.sections[section].selected_path = None;
                ClickAction::HeaderActivated(section)
            }
            MultiSectionViewHit::Body { section } => {
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
                let inner_x = x as f32 - body_b.x;
                let inner_y = y as f32 - body_b.y;
                match inner.hit_test(inner_x, inner_y) {
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
            _ => ClickAction::None,
        }
    }

    /// Apply a mouse-move during an active scrollbar drag. Updates ONLY
    /// the dragged section's `scroll_offset`; returns `true` if any
    /// state changed (caller redraws). 1 cell of drag = 1 row of scroll
    /// — backends that want proportional dragging swap this for
    /// [`quadraui::fit_thumb`] arithmetic.
    ///
    /// Clamps to `[0, rows.len() - viewport_rows]` (the natural max),
    /// NOT `[0, rows.len() - 1]`. Past the natural max the thumb
    /// would saturate at the bottom of the gutter (per `fit_thumb`'s
    /// `clamp(scroll/range, 0, 1)`), but `TreeView`'s internal scroll
    /// renderer would keep going — yielding states where only the
    /// trailing row is visible while the thumb sits idle. The natural
    /// clamp eliminates that mode by construction.
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

    /// Release the captured drag.
    pub fn drag_end(&mut self) {
        self.scroll_drag = None;
    }

    /// Page-scroll a section by `delta` rows (negative = up, positive
    /// = down). Used by track-page handling: clicking the gutter above
    /// the thumb pages up by viewport rows; below the thumb pages down.
    /// Clamps to `[0, rows.len() - viewport_rows]` so paging past the
    /// last viewport-full of rows is impossible.
    fn page_scroll(&mut self, section: usize, delta: isize, viewport_rows: usize) {
        let row_count = self.sections[section].rows.len();
        let max = row_count.saturating_sub(viewport_rows) as isize;
        let cur = self.sections[section].scroll_offset as isize;
        let new = (cur + delta).max(0).min(max) as usize;
        self.sections[section].scroll_offset = new;
    }

    /// Cycle the active section by `delta` (`+1` Tab, `-1` Shift+Tab).
    /// Wraps around. Clears prior selection by design — Debug sidebar
    /// treats Tab as "move focus to next pane" not "preserve selection".
    pub fn cycle_active(&mut self, delta: isize) {
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

    /// Scroll the active section by `delta` rows. Clamps to the
    /// natural max (`rows.len() - viewport_rows`). `area` is the
    /// current sidebar render rect — needed to compute the active
    /// section's viewport row count via `tui_msv_layout`.
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

    /// Select first row of the active section (Enter shortcut).
    pub fn select_first_of_active(&mut self) {
        let Some(idx) = self.active_section else {
            return;
        };
        if let Some(first) = self.sections[idx].rows.first() {
            self.sections[idx].selected_path = Some(first.path.clone());
        }
    }
}

impl Default for DebugSidebar {
    fn default() -> Self {
        Self::new()
    }
}

/// What [`DebugSidebar::click`] decided. The example uses these only
/// for status text; consumer apps can pattern-match further.
#[derive(Debug, Clone, PartialEq)]
pub enum ClickAction {
    HeaderActivated(usize),
    BodyActivated(usize),
    RowSelected { section: usize, path: TreePath },
    ScrollbarPressed(usize),
    ScrollbarPagedUp(usize),
    ScrollbarPagedDown(usize),
    None,
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

// ── Runner ─────────────────────────────────────────────────────────────────

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let theme = Theme::default();
    let mut sidebar = DebugSidebar::new();
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
    sidebar: &mut DebugSidebar,
    last_action: &mut Option<ClickAction>,
) -> io::Result<()> {
    loop {
        let mut sidebar_area = Rect::default();
        terminal.draw(|frame| {
            let size = frame.area();
            // Reserve bottom row for a status line.
            sidebar_area = Rect::new(0, 0, size.width, size.height.saturating_sub(1));

            let view = sidebar.build_view();
            draw_multi_section_view(
                frame.buffer_mut(),
                sidebar_area,
                &view,
                theme,
                /* nerd_fonts */ false,
            );

            // Status line.
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

        // Block up to 200ms for an event; on timeout just redraw.
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Tab => sidebar.cycle_active(1),
                KeyCode::BackTab => sidebar.cycle_active(-1),
                KeyCode::Up => {
                    sidebar.scroll_active(sidebar_area, -1);
                }
                KeyCode::Down => {
                    sidebar.scroll_active(sidebar_area, 1);
                }
                KeyCode::Enter => sidebar.select_first_of_active(),
                _ => {}
            },
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

fn format_status(sidebar: &DebugSidebar, last: Option<&ClickAction>) -> String {
    let active = match sidebar.active_section {
        Some(i) => sidebar.sections[i].id.clone(),
        None => "<none>".to_string(),
    };
    let action = match last {
        Some(ClickAction::HeaderActivated(i)) => format!("header→{}", sidebar.sections[*i].id),
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
    format!(" active: {active}  last: {action}  (Tab/↑↓/click/drag, q quit) ")
}
